module Bot::LimitOrderable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :limit_ordered,
                   :limit_order_pcnt_distance

    after_initialize :initialize_limit_orderable_settings

    validates :limit_ordered, inclusion: { in: [true, false] }
    validates :limit_order_pcnt_distance,
              numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 },
              if: :limit_ordered
    validate :limit_ordered_required_for_exchange

    decorators = Module.new do
      def parse_params(params)
        parsed_limit_order_pcnt_distance = params[:limit_order_pcnt_distance].presence&.to_f
        parsed_limit_order_pcnt_distance = ((parsed_limit_order_pcnt_distance / 100).round(4) if parsed_limit_order_pcnt_distance.present?)
        parsed_limit_ordered = limit_orders_locked? || params[:limit_ordered].presence&.in?(%w[1 true])
        super(params).merge(
          limit_ordered: parsed_limit_ordered,
          limit_order_pcnt_distance: parsed_limit_order_pcnt_distance
        ).compact
      end

      def execute_action
        Bot::FetchAndUpdateOpenOrdersJob.perform_now(self, update_missed_quote_amount: true) if transactions.open.any?

        super
      end

      def exchange=(value)
        super
        self.limit_ordered = true if value.is_a?(Exchanges::Hyperliquid)
      end
    end

    prepend decorators
  end

  def limit_ordered?
    limit_orders_locked? || limit_ordered == true
  end

  def limit_orders_locked?
    exchange.is_a?(Exchanges::Hyperliquid)
  end

  private

  def limit_order_pcnt_distance_decimal
    value = limit_order_pcnt_distance
    return BigDecimal('0.001') if value.blank?

    value.to_d
  end

  def initialize_limit_orderable_settings
    self.limit_ordered ||= false
    self.limit_order_pcnt_distance ||= 0.001
  end

  def limit_ordered_required_for_exchange
    return unless limit_orders_locked?
    return unless limit_ordered == false

    errors.add(:limit_ordered, :inclusion)
  end
end
