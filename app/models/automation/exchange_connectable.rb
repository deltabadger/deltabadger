module Automation::ExchangeConnectable
  extend ActiveSupport::Concern

  included do
    belongs_to :exchange, optional: true

    # ONE registration on purpose. ActiveSupport::Callbacks keys callbacks by filter, so declaring
    # the same method twice (once `on: :start`, once with an `if:`) would silently replace the
    # first rather than stack — and both triggers are needed: bots run valid?(:start) through
    # Bot::Lifecycle#start, while Rules::Withdrawal#start calls a plain update! that never enters
    # the :start context.
    validate :validate_exchange_not_retired
    validate :validate_exchange_order_placement_available
  end

  def api_key
    @api_key ||= user.api_keys.find_by(exchange_id:, key_type: api_key_type) ||
                 user.api_keys.new(exchange_id:, key_type: api_key_type, status: :pending_validation)
  end

  def ensure_exchange_authenticated
    exchange.set_client(api_key: api_key) if exchange.present? && (exchange.api_key.blank? || exchange.api_key != api_key)
  end

  private

  # Stopping and deleting stay possible — neither status is `working?` — so a stranded automation
  # can still be wound down or moved to a live exchange.
  def validate_exchange_not_retired
    return unless exchange&.retired?
    return unless validation_context == :start || (will_save_change_to_status? && working?)

    errors.add(:base, I18n.t('errors.exchange_retired'))
  end

  # The Windows bundle ships without hyperliquid-rb, so its order placement fails on every
  # interval. The exchange picker disables the button, but that is a view: the REST API, the MCP
  # tools and a crafted wizard post all reach the same models. Same trigger shape as the retired
  # guard — a bot may exist, it may not be put to work.
  def validate_exchange_order_placement_available
    return if exchange.nil? || exchange.order_placement_available?
    return unless validation_context == :start || (will_save_change_to_status? && working?)

    errors.add(:base, I18n.t('bot.hyperliquid_trading_unavailable'))
  end

  def with_api_key
    ensure_exchange_authenticated
    yield
  end
end
