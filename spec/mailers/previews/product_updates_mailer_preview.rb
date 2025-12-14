# Preview all emails at http://localhost:3000/rails/mailers/product_updates_mailer
class ProductUpdatesMailerPreview < ActionMailer::Preview
  def bot_goes_opensource
    ProductUpdatesMailer.bot_goes_opensource(mock_mailing_with_content_key('bot_goes_opensource'))
  end

  private

  def mock_mailing_with_content_key(content_key)
    # Create a mock user with current locale
    user = User.new(
      email: 'test@example.com',
      name: 'Mathias',
      locale: params[:locale] || I18n.default_locale
    )

    # Create a mock campaign subscription with token
    campaign_subscription = Struct.new(:token).new('mock-token-123')

    # Create a mock subscription
    subscription = Struct.new(:subscriber, :caffeinate_campaign_subscription).new(
      user,
      campaign_subscription
    )

    # Create a mock drip with options
    drip = Struct.new(:options).new({ content_key: })

    # Create a mock mailing with the required attributes
    Struct.new(:subscription, :subscriber, :mailer_action, :caffeinate_campaign_subscription, :drip).new(
      subscription,
      user,
      content_key,
      campaign_subscription,
      drip
    )
  end
end
