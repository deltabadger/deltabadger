class Payments::Zen < Payment
  include Rails.application.routes.url_helpers

  HOST = ENV.fetch('APP_ROOT_URL').freeze
  ZEN_IPN_SECRET = ENV.fetch('ZEN_IPN_SECRET').freeze

  # validates :first_name, :last_name, presence: true

  def self.valid_ipn_params?(params)
    message = [
      params[:merchantTransactionId],
      params[:currency],
      params[:amount],
      params[:status],
      ZEN_IPN_SECRET
    ].compact.join
    message_hash = Digest::SHA256.hexdigest(message).upcase
    message_hash == params[:hash]
  end

  def get_new_payment_data(locale: nil)
    price = format('%0.02f', total)
    result = client.checkout(
      amount: price,
      currency: currency.upcase,
      merchant_transaction_id: id.to_s,
      customer_first_name: first_name,
      customer_last_name: last_name,
      customer_email: user.email,
      item_name: item_description,
      item_price: price,
      item_quantity: 1,
      item_line_amount_total: price,
      billing_address_country_state: country,
      # specified_payment_method: 'PME_CARD',
      # specified_payment_channel: 'PCL_CARD',
      url_success: upgrade_zen_payment_success_url(host: HOST, locale: locale || I18n.locale),
      url_failure: upgrade_zen_payment_failure_url(host: HOST, locale: locale || I18n.locale),
      custom_ipn_url: upgrade_zen_payment_ipn_url(host: HOST, locale: locale || I18n.locale),
      language: locale
    )
    return result if result.failure?

    url = Utilities::Hash.dig_or_raise(result.data, 'redirectUrl')
    data = {
      payment_id: url.split('/').last.split('?').first,
      url: url
    }
    Result::Success.new(data)
  end

  def get_new_recurring_payment_data(locale: nil)
    price = '1.00' # format('%0.02f', total)
    result = client.checkout(
      amount: price,
      currency: currency.upcase,
      merchant_transaction_id: id.to_s,
      customer_first_name: first_name,
      customer_last_name: last_name,
      customer_email: user.email,
      item_name: item_description,
      item_price: price,
      item_quantity: 1,
      item_line_amount_total: price,
      billing_address_country_state: country,
      # specified_payment_method: 'PME_CARD',
      # specified_payment_channel: 'PCL_CARD',
      # recurring_data_payment_type: 'recurring',
      # recurring_data_expiry_date: '99991212',
      # recurring_data_frequency: '1',
      recurring_data_payment_type: 'unscheduled',
      url_success: upgrade_zen_payment_success_url(host: HOST, locale: locale || I18n.locale),
      url_failure: upgrade_zen_payment_failure_url(host: HOST, locale: locale || I18n.locale),
      custom_ipn_url: upgrade_zen_payment_ipn_url(host: HOST, locale: locale || I18n.locale),
      language: locale
    )
    return result if result.failure?

    url = Utilities::Hash.dig_or_raise(result.data, 'redirectUrl')
    data = {
      payment_id: url.split('/').last.split('?').first,
      url: url
    }
    Result::Success.new(data)
  end

  def handle_ipn(params)
    status = parse_payment_status(params[:status])
    return unless status == :paid

    sync_card_info!(params[:cardToken], params[:transactionId])
    update!(
      status: status,
      paid_at: Time.current,
      first_name: params[:customer][:firstName],
      last_name: params[:customer][:lastName]
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
    @client ||= Clients::Zen.new
  end

  def item_description
    "#{subscription_plan.name.capitalize} Plan Upgrade"
  end

  def parse_payment_status(status)
    case status
    when 'ACCEPTED'
      :paid
    else
      :unpaid
    end
  end

  def sync_card_info!(card_token, transaction_id)
    return if card_token.blank?

    card = user.cards.first
    if card.present?
      if card.token == card_token
        return if card.first_transaction_id.present?

        card.update!(first_transaction_id: transaction_id)
      else
        card.destroy!
      end
    end

    user.cards.create!(token: card_token, first_transaction_id: transaction_id)
  end
end
