class Payments::Btcpay < Payment
  include Rails.application.routes.url_helpers

  HOST = ENV.fetch('APP_ROOT_URL').freeze

  validates :first_name, :last_name, presence: true
  validate :requires_minimum_age

  def self.valid_ipn_params?(params)
    data = params['data']
    return false unless data.present?

    payment_id = data['id']
    payment_id.present? && payment_id.in?(Payment.btcpay.pluck(:payment_id))
  end

  def get_new_payment_data(locale: nil)
    result = client.create_invoice(
      price: total.to_s,
      currency: currency.upcase,
      order_id: id,
      buyer_email: user.email,
      buyer_name: "#{first_name} #{last_name}",
      buyer_phone: birth_date,
      buyer_country: country,
      item_desc: item_description,
      redirect_url: payments_btcpay_success_url(host: HOST, locale: locale || I18n.locale),
      notification_url: payments_btcpay_ipn_url(host: HOST, locale: locale || I18n.locale),
      extended_notifications: true
    )
    return result if result.failure?

    data = {
      payment_id: Utilities::Hash.dig_or_raise(result.data, 'id'),
      external_statuses: Utilities::Hash.dig_or_raise(result.data, 'status'),
      btc_total: Utilities::Hash.dig_or_raise(result.data, 'btcPrice'),
      url: Utilities::Hash.dig_or_raise(result.data, 'url')
    }
    Result::Success.new(data)
  end

  def handle_ipn(params)
    data = params.fetch('data')
    external_status = data.fetch('status')
    status = parse_payment_status(external_status)

    update!(
      external_statuses: external_statuses << external_status,
      btc_paid: data.fetch('btcPaid'),
      status: paid? ? :paid : status,
      paid_at: !paid? && status == :paid ? Time.at(data.fetch('currentTime') / 1000) : nil
    )
    user.subscriptions.create!(
      subscription_plan_variant: subscription_plan_variant,
      ends_at: subscription_plan_variant.years.nil? ? nil : paid_at + subscription_plan_variant.duration
    )
    send_invoice
    notify_subscription_granted
  end

  private

  def client
    @client ||= Clients::Btcpay.new
  end

  def requires_minimum_age
    return unless birth_date.nil? || birth_date > 18.years.ago.to_date

    errors.add(:birth_date, 'You must be at least 18 years old.')
  end

  def item_description
    "#{subscription_plan.name.capitalize} Plan Upgrade"
  end

  def parse_payment_status(status)
    case status
    when 'paid', 'confirmed', 'complete'
      :paid
    when 'expired', 'invalid'
      :cancelled
    else
      :unpaid
    end
  end
end
