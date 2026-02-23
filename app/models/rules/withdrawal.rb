class Rules::Withdrawal < Rule
  belongs_to :exchange
  belongs_to :asset

  encrypts :address

  store_accessor :settings, :max_fee_percentage, :network, :address_tag, :threshold_type, :min_amount

  validates :address, presence: true
  validates :max_fee_percentage, presence: true,
                                 numericality: { greater_than: 0, less_than_or_equal_to: 100 },
                                 unless: -> { threshold_type == 'min_amount' }
  validates :min_amount, presence: true,
                         numericality: { greater_than: 0 },
                         if: -> { threshold_type == 'min_amount' }

  def api_key_type = :withdrawal

  def start(_start_fresh: true)
    update!(status: :scheduled)
  end

  def stop(_stop_message_key: nil)
    update!(status: :stopped)
  end

  def delete
    update!(status: :deleted)
  end

  def parse_params(params)
    self.threshold_type = params[:threshold_type] if params.key?(:threshold_type)
    self.max_fee_percentage = params[:max_fee_percentage] if params[:max_fee_percentage].present?
    self.min_amount = params[:min_amount] if params[:min_amount].present?
    self.network = params[:network] if params.key?(:network)
    self.address_tag = params[:address_tag] if params.key?(:address_tag)
  end

  def execute
    balance_result = with_api_key { exchange.get_balance(asset_id: asset_id) }
    if balance_result.failure?
      log_failed("Failed to fetch balance: #{balance_result.errors.first}")
      return balance_result
    end

    free_balance = BigDecimal(balance_result.data[:free].to_s)
    min_amount = minimum_withdrawal_amount

    if min_amount.nil?
      # Fee is zero or unknown — refresh and re-check
      refresh_fee_result = with_api_key { exchange.fetch_withdrawal_fees! }
      if refresh_fee_result.failure?
        log_failed("Failed to refresh withdrawal fees: #{refresh_fee_result.errors.first}")
        return refresh_fee_result
      end
      reload # pick up updated exchange_asset
      min_amount = minimum_withdrawal_amount
    end

    if min_amount.present? && free_balance < min_amount
      log_skipped("Balance #{free_balance} #{asset.symbol} below minimum #{min_amount} #{asset.symbol}")
      return Result::Success.new(skipped: true)
    end

    # Refresh fee if stale
    unless exchange.withdrawal_fee_fresh?(asset: asset)
      refresh_result = with_api_key { exchange.fetch_withdrawal_fees! }
      if refresh_result.failure?
        log_failed("Failed to refresh withdrawal fees: #{refresh_result.errors.first}")
        return refresh_result
      end
      exchange.exchange_assets.reset
    end

    fee = exchange.withdrawal_fee_for(asset: asset) || BigDecimal('0')
    amount = free_balance - fee

    if amount <= 0
      log_skipped("Balance #{free_balance} #{asset.symbol} does not cover fee #{fee} #{asset.symbol}")
      return Result::Success.new(skipped: true)
    end

    # Re-check fee percentage against actual amount
    if fee.positive? && min_amount.present? && free_balance < min_amount
      log_skipped("Balance #{free_balance} #{asset.symbol} below minimum #{min_amount} #{asset.symbol} after fee refresh")
      return Result::Success.new(skipped: true)
    end

    withdraw_result = with_api_key do
      exchange.withdraw(asset: asset, amount: amount, address: address,
                        network: network.presence, address_tag: address_tag.presence)
    end
    if withdraw_result.failure?
      log_failed("Withdrawal failed: #{withdraw_result.errors.first}",
                 details: { amount: amount.to_s('F'), fee: fee.to_s('F'), address: address,
                            network: network, address_tag: address_tag })
      return withdraw_result
    end

    log_success("Withdrew #{amount.to_s('F')} #{asset.symbol}",
                details: {
                  amount: amount.to_s('F'),
                  fee: fee.to_s('F'),
                  address: address,
                  network: network,
                  address_tag: address_tag,
                  withdrawal_id: withdraw_result.data[:withdrawal_id]
                })
    withdraw_result
  end

  def withdrawal_fee_amount
    chain_fee = fee_for_selected_chain
    return BigDecimal(chain_fee) if chain_fee.present?

    exchange.withdrawal_fee_for(asset: asset) || BigDecimal('0')
  end

  def withdrawal_fee_known?
    ExchangeAsset.find_by(exchange: exchange, asset: asset)&.withdrawal_fee.present?
  end

  def available_chains
    ExchangeAsset.find_by(exchange: exchange, asset: asset)&.withdrawal_chains || []
  end

  def minimum_withdrawal_amount
    if threshold_type == 'min_amount'
      return nil if min_amount.blank?

      BigDecimal(min_amount.to_s)
    else
      return nil if max_fee_percentage.blank?

      fee = withdrawal_fee_amount
      pct = BigDecimal(max_fee_percentage.to_s)
      return nil if pct.zero?
      return nil if fee.zero?

      (fee / (pct / 100)).round(8)
    end
  end

  private

  def fee_for_selected_chain
    return nil if network.blank?

    chain = available_chains.find { |c| c['name'] == network }
    chain&.dig('fee')
  end
end
