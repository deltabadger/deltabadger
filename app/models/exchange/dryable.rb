module Exchange::Dryable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def get_balances(asset_ids: nil)
        dry_run? ? get_dry_balances(asset_ids: asset_ids) : super
      end

      def get_order(order_id:)
        dry_run? ? get_dry_order(order_id: order_id) : super
      end

      def get_orders(order_ids:)
        dry_run? ? get_dry_orders(order_ids: order_ids) : super
      end

      def cancel_order(order_id:)
        dry_run? ? dry_cancel_order(order_id: order_id) : super
      end

      def get_api_key_validity(api_key:)
        dry_run? ? Result::Success.new(true) : super
      end

      def set_market_order(ticker:, amount:, amount_type:, side:)
        if dry_run?
          set_dry_market_order(ticker: ticker, amount: amount, amount_type: amount_type,
                               side: side)
        else
          super
        end
      end

      def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
        if dry_run?
          set_dry_limit_order(ticker: ticker, amount: amount, amount_type: amount_type,
                              side: side, price: price)
        else
          super
        end
      end

      def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
        dry_run? ? dry_withdraw(asset:, amount:, address:, network:, address_tag:) : super
      end

      def fetch_withdrawal_fees!
        dry_run? ? dry_fetch_withdrawal_fees! : super
      end
    end

    prepend decorators
  end

  def dry_run?
    Thread.current[:force_dry_run] || Rails.configuration.dry_run
  end

  # Funds an asset nothing has been bought in. The quote currency is only ever spent, never bought,
  # so derived from the ledger it would be zero and every local buy would fail for insufficient
  # funds. Big enough to never be the thing under test.
  DRY_UNTRADED_BALANCE = 1_000_000.to_d

  # What the bots on this exchange actually hold, not a random number.
  #
  # A random balance made the local app untestable in a way that read as product bugs. Sizing that
  # caps at the live balance — Liquidatable#liquidation_order_data, Rebalancer#rebalance_sell_order_data
  # — took min(ledger, rand(100..10_000)), so selling a 15M holding placed an order for 950 of it,
  # then 9,849 on the next click, then 1,009. The position never left the portfolio and nothing said
  # why. It also rolled a FRESH number per call, so two reads within one sale disagreed.
  #
  # The fallback is deliberately NOT handed out on a whole-wallet read. AccountBalance::Sync and
  # BotApi::Exchanges::Balances ask for every asset at once and PERSIST what they get; funding all of
  # them would put a million units of each into the tracker, which on a stock exchange is thousands
  # of assets and a portfolio total that means nothing. A sizing caller always names its asset
  # (Exchange#get_balance passes one id), so only that path is funded.
  def get_dry_balances(asset_ids: nil)
    listing = asset_ids.nil?
    asset_ids ||= assets.pluck(:id)
    held = dry_held_amounts(asset_ids)
    balances = asset_ids.to_h do |asset_id|
      free = held[asset_id] || (listing ? 0.to_d : dry_funding(asset_id))
      [asset_id, { free: free, locked: 0 }]
    end
    Result::Success.new(balances)
  end

  private

  # Only assets something on this venue actually PRICES in. Handing the fallback to any asset we
  # failed to match a ledger row against — a legacy `base` string, say (Transaction still carries
  # BTC = %w[XXBT XBT BTC] and a four-level resolver for exactly that reason) — would size a sale
  # against a million units that do not exist, which is the oversizing mirror of the bug above.
  # Unmatched assets read as zero instead, and a zero skips the sale rather than inventing one.
  def dry_funding(asset_id)
    dry_quote_asset_ids.include?(asset_id) ? DRY_UNTRADED_BALANCE : 0.to_d
  end

  def dry_quote_asset_ids
    @dry_quote_asset_ids ||= tickers.distinct.pluck(:quote_asset_id).compact.to_set
  end

  # COALESCE(amount_exec, amount) over CLOSED rows — the same ledger Bot#total_amount,
  # Bot::BaseAmountLimitable and the metrics all compute. It has to be the same one: these balances
  # are compared against that ledger with min(), so any divergence reappears as a sale that silently
  # covers part of a position the page shows in full.
  #
  # An asset with no rows at all is ABSENT from the result rather than zero, which is what lets the
  # caller tell "never traded" from "sold out".
  def dry_held_amounts(asset_ids)
    ids_by_symbol = Asset.where(id: asset_ids).pluck(:symbol, :id)
                         .group_by(&:first).transform_values { |pairs| pairs.map(&:last) }
    # No IN list on the transactions side: a stock exchange lists ~11.5k assets and there are only a
    # handful of distinct `base` values, so filtering by asset would mean a bind parameter each.
    net = transactions.submitted.closed.group(:base, :side).sum(Arel.sql('COALESCE(amount_exec, amount)'))

    totals = Hash.new(0.to_d)
    net.each { |(base, side), amount| totals[base] += side.to_s == 'sell' ? -amount.to_d : amount.to_d }

    totals.each_with_object({}) do |(base, amount), acc|
      # Symbols are NOT unique — a Hyperliquid RWA and the stock it tracks share one, and the FIGI
      # work split them into separate rows on purpose. A transaction records only the symbol, so
      # neither asset can claim the ledger alone and both report the same figure; dividing it would
      # be inventing a split the ledger does not record.
      ids_by_symbol.fetch(base, []).each { |id| acc[id] = [amount, 0.to_d].max }
    end
  end

  def get_dry_order(order_id:)
    order_data = Rails.cache.read(order_id)
    return Result::Failure.new("Dry order #{order_id} not found") if order_data.blank?

    order_data[:ticker] = Ticker.find(order_data[:ticker_id])
    order_data.delete(:ticker_id)

    Rails.cache.delete(order_id)
    Result::Success.new(order_data)
  end

  # An id with no dry record goes in `missing`, which is the contract every real client honours —
  # it is not a failure of the whole call. Returning a Failure for the batch meant ONE unknown id
  # (a seeded row, an entry that has been read once and deleted) poisoned every sweep on that bot:
  # Bot::FetchAndUpdateOpenOrdersJob raises on a failed fetch, so no waiting order anywhere in the
  # batch could advance, and a locally-seeded bot's orders sat open forever.
  def get_dry_orders(order_ids:)
    orders = {}
    missing = []
    order_ids.each do |order_id|
      result = get_dry_order(order_id: order_id)
      if result.failure?
        missing << order_id
      else
        orders[order_id] = result.data
      end
    end

    Result::Success.new(orders: orders, missing: missing)
  end

  def dry_withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    Result::Success.new({ withdrawal_id: "dry-withdrawal-#{SecureRandom.uuid}" })
  end

  def dry_fetch_withdrawal_fees!
    exchange_assets.find_each do |ea|
      ea.update!(
        withdrawal_fee: '0.001',
        withdrawal_fee_updated_at: Time.current,
        withdrawal_chains: [{ 'name' => 'DryNet', 'fee' => '0.001', 'is_default' => true }]
      )
    end
    Result::Success.new({})
  end

  def dry_cancel_order(order_id:)
    raise StandardError, 'Dry run mode does not support cancel_order'
  end

  def set_dry_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    result = side == :buy ? get_ask_price(ticker: ticker) : get_bid_price(ticker: ticker)
    price = result.success? ? result.data : nil

    create_dry_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      order_type: :market_order,
      side: side,
      price: price
    )
  end

  def set_dry_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    price = ticker.adjusted_price(price: price)

    create_dry_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      order_type: :limit_order,
      side: side,
      price: price
    )
  end

  def create_dry_order(ticker:, amount:, amount_type:, order_type:, side:, price: nil)
    dry_order_id = "dry-order-#{SecureRandom.uuid[10...]}"
    base_amount = amount_type == :base ? amount : nil
    quote_amount = amount_type == :quote ? amount : nil

    # mocks get_order response
    dry_order_data = {
      order_id: dry_order_id,

      # We can't store the ticker object because we're in a concern added to the exchange-specific singleton_module
      # and Rails Marshal doesn't know how to serialize it when writing into the cache, so we store the id instead
      ticker_id: ticker.id,

      price: price,
      amount: base_amount,
      quote_amount: quote_amount,
      side: side,
      order_type: order_type,
      amount_exec: base_amount || (quote_amount / price if price.present?),
      quote_amount_exec: quote_amount || (amount * price if price.present?),
      error_messages: [],
      status: :closed,
      exchange_response: {}
    }
    Rails.cache.write(dry_order_id, dry_order_data)

    # mocks set_market_order/set_limit_order response
    set_dry_order_data = {
      order_id: dry_order_id
    }
    Result::Success.new(set_dry_order_data)
  end
end
