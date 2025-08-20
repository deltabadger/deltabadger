# Preview all emails at http://localhost:3000/rails/mailers/onboarding_mailer
class OnboardingMailerPreview < ActionMailer::Preview
  def onboarding_fee_cutter
    OnboardingMailer.onboarding(mock_mailing_with_content_key('fee_cutter'))
  end

  def onboarding_referral
    OnboardingMailer.onboarding_referral(mock_mailing_for_referral)
  end

  def onboarding_rsi
    OnboardingMailer.onboarding(mock_mailing_with_content_key('rsi'))
  end

  def onboarding_avoid_taxes
    OnboardingMailer.onboarding(mock_mailing_with_content_key('avoid_taxes'))
  end

  def onboarding_bitcoin_m2
    OnboardingMailer.onboarding(mock_mailing_with_content_key('bitcoin_m2'))
  end

  def onboarding_grayscale_etf
    OnboardingMailer.onboarding(mock_mailing_with_content_key('grayscale_etf'))
  end

  def onboarding_stablecoins
    OnboardingMailer.onboarding(mock_mailing_with_content_key('stablecoins'))
  end

  def onboarding_polymarket
    OnboardingMailer.onboarding(mock_mailing_with_content_key('polymarket'))
  end

  private

  def mock_mailing_with_content_key(content_key)
    # Create a mock user
    user = User.new(
      email: 'test@example.com',
      name: 'John Doe'
    )

    # Create a mock campaign subscription with token
    campaign_subscription = Struct.new(:token).new('mock-token-123')

    # Create a mock subscription
    subscription = Struct.new(:subscriber, :caffeinate_campaign_subscription).new(
      user, 
      campaign_subscription
    )

    # Create a mock drip with options
    drip = Struct.new(:options).new({ content_key: content_key })

    # Create a mock mailing with the required attributes
    Struct.new(:subscription, :subscriber, :mailer_action, :caffeinate_campaign_subscription, :drip).new(
      subscription,
      user,
      'onboarding',
      campaign_subscription,
      drip
    )
  end

  def mock_mailing
    mock_mailing_with_content_key('fee_cutter')
  end

  def mock_mailing_for_referral
    # Create a mock user with affiliate
    user = User.new(
      email: 'test@example.com',
      name: 'John Doe'
    )
    
    # Create a mock affiliate with code
    affiliate = Struct.new(:code).new('ABC123')
    user.define_singleton_method(:affiliate) { affiliate }

    # Create a mock campaign subscription with token
    campaign_subscription = Struct.new(:token).new('mock-token-123')

    # Create a mock subscription
    subscription = Struct.new(:subscriber, :caffeinate_campaign_subscription).new(
      user, 
      campaign_subscription
    )

    # Create a mock mailing with the required attributes (no drip needed for referral)
    Struct.new(:subscription, :subscriber, :mailer_action, :caffeinate_campaign_subscription).new(
      subscription,
      user,
      'onboarding_referral',
      campaign_subscription
    )
  end
end
