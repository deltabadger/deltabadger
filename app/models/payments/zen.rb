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
    result = checkout_client.checkout(
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
      # billing_address_country_state: country,
      # specified_payment_method: 'PME_CARD',
      # specified_payment_channel: 'PCL_CARD',
      url_success: upgrade_zen_payment_success_url(host: HOST, locale: locale || I18n.locale),
      url_failure: upgrade_zen_payment_failure_url(host: HOST, locale: locale || I18n.locale),
      custom_ipn_url: upgrade_zen_payment_ipn_url(host: HOST),
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
    result = checkout_client.checkout(
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
      # billing_address_country_state: country,
      # recurring_data_payment_type: 'recurring',
      # recurring_data_expiry_date: '99991212',
      # recurring_data_frequency: '1',
      recurring_data_payment_type: 'unscheduled',
      url_success: upgrade_zen_payment_success_url(host: HOST, locale: locale || I18n.locale),
      url_failure: upgrade_zen_payment_failure_url(host: HOST, locale: locale || I18n.locale),
      custom_ipn_url: upgrade_zen_payment_ipn_url(host: HOST),
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

  def charge_next_recurring_payment
    price = format('%0.02f', total)
    item_description = 'Test Renewal'
    result = api_client.create_purchase_transaction(
      amount: price,
      currency: currency.upcase,
      merchant_transaction_id: (id + 1).to_s,
      payment_channel: 'PCL_CARD',
      customer_first_name: 'Jan', # first_name,
      customer_last_name: 'Klosowski', # last_name,
      customer_email: 'guillemavila+6@gmail.com', # user.email,
      customer_ip: '194.127.167.81',
      item_name: item_description,
      item_price: price,
      item_quantity: 1,
      item_line_amount_total: price,
      # billing_address_country_state: country,
      # payment_specific_data_payment_type: 'recurring_token',
      payment_specific_data_payment_type: 'unscheduled_token',
      payment_specific_data_card_token: 'b9441b40-a127-4f17-9d2d-4d9ae8a8e497', # user.cards.first.token,
      payment_specific_data_first_transaction_id: '4aa1e8b5-f1fd-4019-8a02-f48c15dd7767', # user.cards.first.first_transaction_id,
      payment_specific_data_descriptor: item_description.upcase.gsub(/\s+/, '_'),
      custom_ipn_url: 'https://test.deltabadger.com/upgrade/zen_payment/ipn' #  upgrade_zen_payment_ipn_url(host: HOST)
    )
    return result if result.failure?

    Result::Success.new(result.data)
  end

  def handle_ipn(params)
    status = parse_payment_status(params[:status])
    return unless status == :paid

    sync_card_info!(params[:cardToken], params[:transactionId], params[:customer][:ip]) if recurring?
    update!(
      status: status,
      paid_at: Time.current,
      first_name: params[:customer][:firstName],
      last_name: params[:customer][:lastName]
    )
    grant_subscription
    send_invoice
    notify_subscription_granted if subscription_plan.name != user.subscription.name
  end

  private

  def api_client
    @api_client ||= Clients::Zen.new
  end

  def checkout_client
    @checkout_client ||= Clients::ZenCheckout.new
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

  def sync_card_info!(card_token, transaction_id, ip)
    return if card_token.blank?

    card = user.cards.first
    if card.present?
      if card.token == card_token
        return if card.first_transaction_id.present?

        card.update!(first_transaction_id: transaction_id, ip: ip)
      else
        card.destroy!
      end
    end

    user.cards.create!(token: card_token, first_transaction_id: transaction_id, ip: ip)
  end
end
