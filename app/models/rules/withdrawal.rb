class Rules::Withdrawal < Rule
  belongs_to :exchange
  belongs_to :asset

  encrypts :address

  before_validation :default_withdrawal_percentage

  store_accessor :settings, :max_fee_percentage, :network, :address_tag, :threshold_type, :min_amount, :address_name,
                 :max_interval, :withdrawal_percentage

  validates :address, presence: true
  validates :withdrawal_percentage, numericality: { greater_than: 0, less_than_or_equal_to: 100 },
                                    if: -> { scheduled? }
  validates :max_fee_percentage, presence: true,
                                 numericality: { greater_than: 0, less_than_or_equal_to: 100 },
                                 unless: -> { threshold_type == 'min_amount' },
                                 if: -> { scheduled? }
  validates :min_amount, presence: true,
                         numericality: { greater_than: 0 },
                         if: -> { threshold_type == 'min_amount' && scheduled? }

  def api_key_type = :withdrawal

  def start(_start_fresh: true)
    update!(status: :scheduled)
    Rule::EvaluateAllJob.perform_later
  end

  def stop(_stop_message_key: nil)
    update!(status: :stopped)
  end

  def delete
    update!(status: :deleted)
  end

  def parse_params(params)
    self.withdrawal_percentage = params[:withdrawal_percentage] if params[:withdrawal_percentage].present?
    self.threshold_type = params[:threshold_type] if params.key?(:threshold_type)
    self.max_fee_percentage = params[:max_fee_percentage] if params[:max_fee_percentage].present?
    self.min_amount = params[:min_amount] if params[:min_amount].present?
    self.max_interval = params[:max_interval].presence if params.key?(:max_interval)
    self.network = params[:network] if params.key?(:network)
    self.address_tag = params[:address_tag] if params.key?(:address_tag)
    self.address = params[:address] if params[:address].present?
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
        log_failed("Failed to refresh withdrawal fees: #{refresh_fee_result.errors.first}",
                   details: { free_balance: free_balance.to_s('F') })
        return refresh_fee_result
      end
      reload # pick up updated exchange_asset
      min_amount = minimum_withdrawal_amount

      if min_amount.nil? && threshold_type != 'min_amount'
        log_skipped("Withdrawal fee unknown for #{asset.symbol} — cannot evaluate fee percentage threshold",
                    details: { free_balance: free_balance.to_s('F') })
        return Result::Success.new(skipped: true)
      end
    end

    interval_bypass = max_interval_elapsed?

    if min_amount.present? && free_balance < min_amount && !interval_bypass
      log_skipped("Balance #{free_balance} #{asset.symbol} below minimum #{min_amount} #{asset.symbol}",
                  details: { free_balance: free_balance.to_s('F') })
      return Result::Success.new(skipped: true)
    end

    # Refresh fee if stale
    unless exchange.withdrawal_fee_fresh?(asset: asset)
      refresh_result = with_api_key { exchange.fetch_withdrawal_fees! }
      if refresh_result.failure?
        log_failed("Failed to refresh withdrawal fees: #{refresh_result.errors.first}",
                   details: { free_balance: free_balance.to_s('F') })
        return refresh_result
      end
      exchange.exchange_assets.reset
    end

    fee = exchange.withdrawal_fee_for(asset: asset) || BigDecimal('0')
    amount_before_fee = withdrawal_amount_before_fee(free_balance)
    amount = amount_before_fee - fee

    if amount <= 0
      log_skipped("Withdrawal amount #{amount_before_fee.to_s('F')} #{asset.symbol} does not cover fee #{fee} #{asset.symbol}",
                  details: { free_balance: free_balance.to_s('F'),
                             withdrawal_percentage: effective_withdrawal_percentage.to_s('F') })
      return Result::Success.new(skipped: true)
    end

    # Re-check fee percentage against actual amount
    if fee.positive? && min_amount.present? && free_balance < min_amount && !interval_bypass
      log_skipped("Balance #{free_balance} #{asset.symbol} below minimum #{min_amount} #{asset.symbol} after fee refresh",
                  details: { free_balance: free_balance.to_s('F') })
      return Result::Success.new(skipped: true)
    end

    withdraw_result = with_api_key do
      exchange.withdraw(asset: asset, amount: amount, address: address,
                        network: network.presence, address_tag: address_tag.presence)
    end
    if withdraw_result.failure?
      log_failed("Withdrawal failed: #{withdraw_result.errors.first}",
                 details: { free_balance: free_balance.to_s('F'), amount: amount.to_s('F'),
                            fee: fee.to_s('F'), address: address,
                            network: network, address_tag: address_tag,
                            withdrawal_percentage: effective_withdrawal_percentage.to_s('F') })
      return withdraw_result
    end

    log_success("Withdrew #{amount.to_s('F')} #{asset.symbol}",
                details: {
                  free_balance: free_balance.to_s('F'),
                  amount: amount.to_s('F'),
                  fee: fee.to_s('F'),
                  address: address,
                  network: network,
                  address_tag: address_tag,
                  withdrawal_percentage: effective_withdrawal_percentage.to_s('F'),
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
    withdrawal_fraction = effective_withdrawal_percentage / 100
    return nil unless withdrawal_fraction.positive?

    if threshold_type == 'min_amount'
      return nil if min_amount.blank?

      (BigDecimal(min_amount.to_s) / withdrawal_fraction).round(8)
    else
      return nil if max_fee_percentage.blank?

      fee = withdrawal_fee_amount
      pct = BigDecimal(max_fee_percentage.to_s)
      return nil if pct.zero?
      return nil if fee.zero?

      ((fee / (pct / 100)) / withdrawal_fraction).round(8)
    end
  end

  def effective_withdrawal_percentage
    return BigDecimal('100') if withdrawal_percentage.blank?

    BigDecimal(withdrawal_percentage.to_s)
  end

  def max_interval_elapsed?
    return false if max_interval.blank?

    last_success = rule_logs.where(status: :success).order(created_at: :desc).first
    return true unless last_success

    last_success.created_at < max_interval.to_i.days.ago
  end

  def last_known_balance
    log = rule_logs.order(created_at: :desc).first
    return nil unless log
    return BigDecimal('0') if log.success?

    return BigDecimal(log.details['free_balance']) if log.details&.dig('free_balance').present?

    match = log.message&.match(/^Balance (\d+\.?\d*)/)
    BigDecimal(match[1]) if match
  end

  private

  def default_withdrawal_percentage
    self.withdrawal_percentage = '100' if withdrawal_percentage.blank?
  end

  def fee_for_selected_chain
    return nil if network.blank?

    chain = available_chains.find { |c| c['name'] == network }
    chain&.dig('fee')
  end

  def withdrawal_amount_before_fee(free_balance)
    (free_balance * (effective_withdrawal_percentage / 100)).round(8)
  end
end
