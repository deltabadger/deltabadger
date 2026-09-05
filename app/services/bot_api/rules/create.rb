# frozen_string_literal: true

module BotApi
  module Rules
    # Builds a withdrawal rule the way the wizard does, stopped, for `start_rule` to activate.
    #
    # The destination is only acceptable if the exchange itself lists it — the same rule the
    # wizard and the edit path enforce, and it fails closed: an exchange that cannot be asked
    # has not said yes. The refusal says no and nothing more; the caller supplied the address.
    class Create
      REQUIRED = %i[exchange_name asset address].freeze

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      def initialize(user:, exchange_name: nil, asset: nil, address: nil, address_tag: nil, network: nil,
                     withdrawal_percentage: nil, threshold_type: nil, max_fee_percentage: nil, min_amount: nil)
        @user = user
        @exchange_name = exchange_name
        @asset = asset
        @address = address.to_s
        @address_tag = address_tag
        @network = network
        @withdrawal_percentage = withdrawal_percentage
        @threshold_type = threshold_type
        @max_fee_percentage = max_fee_percentage
        @min_amount = min_amount
      end

      def call
        missing = REQUIRED.select { |k| instance_variable_get("@#{k}").blank? }
        if missing.any?
          return Result.failure(:validation_failed, 'missing_required_parameter',
                                "Missing required parameter(s): #{missing.join(', ')}.")
        end

        # tradeable = available and not retired: exactly what the wizard's exchange picker lists.
        exchange = Exchange.tradeable.where('LOWER(name) = ?', @exchange_name.to_s.downcase).first
        unless exchange
          return Result.failure(:not_found, 'exchange_not_found',
                                "Exchange '#{@exchange_name}' not found. Available: #{Exchange.tradeable.pluck(:name).join(', ')}")
        end

        unless exchange.supports_withdrawal?
          return Result.failure(:validation_failed, 'withdrawal_unsupported',
                                "#{exchange.name} does not support withdrawals here.")
        end

        api_key = @user.api_keys.find_by(exchange: exchange, key_type: :withdrawal, status: :correct)
        unless api_key
          return Result.failure(:permission_denied, 'withdrawal_key_missing',
                                "No valid withdrawal API key for #{exchange.name}. Add one in Settings.")
        end

        asset = find_asset(exchange)
        return Result.failure(:not_found, 'asset_not_found', "Asset '#{@asset}' is not listed on #{exchange.name}.") unless asset

        existing = @user.rules.find_by(type: 'Rules::Withdrawal', asset: asset, exchange: exchange)
        if existing && !existing.deleted?
          return Result.failure(:conflict, 'rule_exists',
                                "A withdrawal rule for #{asset.symbol} on #{exchange.name} already exists (##{existing.id}).",
                                data: { id: existing.id })
        end

        exchange.set_client(api_key: api_key)
        # Says no, and only no. The allow-list is the exchange's address book; a refusal that
        # returned it would hand every destination to any client holding this one tool.
        selected = Array(exchange.list_withdrawal_addresses(asset: asset)).find { |row| row[:name] == @address }
        unless selected
          return Result.failure(:validation_failed, 'address_not_listed',
                                "Address is not in #{exchange.name}'s withdrawal allow-list for #{asset.symbol}. " \
                                'Add it on the exchange first.')
        end

        # The wizard's fee-aware default: a known fee means a fee threshold, an unknown one a
        # minimum amount. Refreshed the same way the wizard refreshes before its preview.
        exchange.fetch_withdrawal_fees! unless exchange.withdrawal_fee_fresh?(asset: asset)
        rule = existing || @user.rules.build(type: 'Rules::Withdrawal', asset: asset, exchange: exchange)
        threshold_type = @threshold_type.presence || (rule.withdrawal_fee_known? ? 'fee_percentage' : 'min_amount')
        # Checked with the wizard's defaults filled in — Bounds requires the active threshold's
        # own field, and the defaults are what satisfy it when the caller gave none.
        err = Bounds.check(threshold_type: threshold_type, withdrawal_percentage: number(@withdrawal_percentage, 100),
                           max_fee_percentage: number(@max_fee_percentage, 0.5), min_amount: number(@min_amount, 0.1))
        return err if err

        network = network_for(rule)
        return network if network.is_a?(Result)

        # Every attribute is written, including the optional ones as nil: a revived rule must not
        # carry a max_interval or tag from the life it had before it was deleted.
        rule.assign_attributes(
          address: @address, address_name: selected[:key], address_tag: @address_tag.presence, network: network,
          withdrawal_percentage: number(@withdrawal_percentage, 100),
          threshold_type: threshold_type,
          max_fee_percentage: number(@max_fee_percentage, 0.5),
          min_amount: number(@min_amount, 0.1),
          max_interval: nil,
          status: :created
        )
        if rule.save
          Result.success(List.row_for(rule), status: :created)
        else
          Result.failure(:validation_failed, 'rule_save_failed',
                         "Failed to create rule: #{rule.errors.full_messages.join(', ')}")
        end
      rescue ActiveRecord::RecordNotUnique
        # Two creates raced for the same asset on the same venue; the other one won.
        Result.failure(:conflict, 'rule_exists',
                       "A withdrawal rule for #{@asset.to_s.upcase} on #{@exchange_name} already exists.")
      end

      private

      # The asset as this venue knows it: a base of some tradeable pair first, else a quote (USDT
      # is usually only ever a quote). Symbols are not unique across the whole asset table, and
      # trading_enabled is what the wizard's asset picker filters on.
      def find_asset(exchange)
        symbol = @asset.to_s.upcase
        # One association at a time, so each join keeps the plain `assets` table name — the
        # `quote_assets_tickers` alias only appears when both are joined in the same query.
        tickers = exchange.tickers.available.trading_enabled
        tickers.joins(:base_asset).find_by(assets: { symbol: symbol })&.base_asset ||
          tickers.joins(:quote_asset).find_by(assets: { symbol: symbol })&.quote_asset
      end

      # The wizard's rule: the given chain, else the venue's default chain, else its first. A
      # given chain the venue does not list is refused — the wizard only ever offers listed ones.
      def network_for(rule)
        chains = Array(rule.available_chains)
        names = chains.filter_map { |chain| chain['name'] }
        if @network.present?
          return @network if names.include?(@network)

          # Refused even when the venue lists nothing: the wizard's select is built from this
          # list, so a network it does not contain is not one the wizard could have offered.
          message = if names.any?
                      "network must be one of: #{names.join(', ')}."
                    else
                      "#{rule.exchange.name} lists no networks for #{rule.asset.symbol}; omit network."
                    end
          return Result.failure(:validation_failed, 'invalid_network', message)
        end
        return nil if names.empty?

        chains.find { |chain| chain['is_default'] }&.dig('name') || names.first
      end

      # Stored as strings, like the wizard stores them.
      def number(value, default) = (value.presence || default).to_s
    end
  end
end
