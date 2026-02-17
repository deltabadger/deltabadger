module Automation::ExchangeConnectable
  extend ActiveSupport::Concern

  included do
    belongs_to :exchange, optional: true
  end

  def api_key
    @api_key ||= user.api_keys.find_by(exchange_id:, key_type: api_key_type) ||
                 user.api_keys.new(exchange_id:, key_type: api_key_type, status: :pending_validation)
  end

  private

  def with_api_key
    exchange.set_client(api_key: api_key) if exchange.present? && (exchange.api_key.blank? || exchange.api_key != api_key)
    yield
  end
end
