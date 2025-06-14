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
    validate :validate_limit_orderable_included_in_subscription_plan, on: :start

    decorators = Module.new do
      def parse_params(params)
        parsed_limit_order_pcnt_distance = params[:limit_order_pcnt_distance].presence&.to_f
        parsed_limit_order_pcnt_distance = if parsed_limit_order_pcnt_distance.present?
                                             (parsed_limit_order_pcnt_distance / 100).round(4)
                                           end
        super(params).merge(
          limit_ordered: params[:limit_ordered].presence&.in?(%w[1 true]),
          limit_order_pcnt_distance: parsed_limit_order_pcnt_distance
        ).compact
      end
    end

    prepend decorators
  end

  def limit_ordered?
    limit_ordered == true
  end

  private

  def validate_limit_orderable_included_in_subscription_plan
    return unless limit_ordered?
    return if user.subscription.paid?

    errors.add(:user, :upgrade)
  end

  def initialize_limit_orderable_settings
    self.limit_ordered ||= false
    self.limit_order_pcnt_distance ||= 0.001
  end
end
