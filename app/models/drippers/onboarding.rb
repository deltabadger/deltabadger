class Drippers::Onboarding < Dripper
  self.campaign = :onboarding
  default mailer: 'OnboardingMailer'

  # WARNING: after adding a new drip step, you need to call rake caffeinate_add_drip_step
  # WARNING: after renaming a drip step, you need to call rake caffeinate_rename_drip

  on_resubscribe do |subscription|
    user = subscription.subscriber
    user.update(subscribed_to_email_marketing: true)
  end

  before_drip do |_drip, mailing|
    user = mailing.subscription.subscriber
    unless user.subscribed_to_email_marketing?
      mailing.subscription.unsubscribe!('Not subscribed to email marketing')
      throw(:abort)
    end
  end

  # Optimized sequence for maximum sales + referral impact
  drip :fee_cutter, delay: 1.minute       # Email #1: Immediate value, builds trust
  drip :rsi, delay: 1.day                 # Email #2: Educational value, positions expertise
  drip :referral, delay: 2.days           # Email #3: High engagement, passive income hook
  drip :bitcoin_m2, on: :weekly_sunday     # Email #4: Knowledge article on Sunday
  drip :grayscale_etf, on: :weekly_sunday  # Email #5: Knowledge article on Sunday
  drip :stablecoins, on: :weekly_sunday    # Email #6: Knowledge article on Sunday
  drip :polymarket, on: :weekly_sunday     # Email #7: Knowledge article on Sunday
  drip :market_cap_weighting, on: :weekly_sunday # Email #8: Knowledge article on Sunday
  drip :radical_portfolio, on: :weekly_sunday    # Email #9: Knowledge article on Sunday
  drip :treasury_companies, on: :weekly_sunday   # Email #10: Knowledge article on Sunday
  drip :avoid_taxes, delay: 7.days # Email #11: Advanced strategy for committed users

  private

  # Maps each article to its week number (starting from 0)
  ARTICLE_SCHEDULE = {
    bitcoin_m2: 0,
    grayscale_etf: 1,
    stablecoins: 2,
    polymarket: 3,
    market_cap_weighting: 4,
    radical_portfolio: 5,
    treasury_companies: 6
  }.freeze

  def weekly_sunday(_drip, mailing)
    article_name = _drip.action
    week_offset = ARTICLE_SCHEDULE[article_name]

    signup_date = mailing.subscription.created_at
    referral_send_date = signup_date + 2.days # When referral actually sends
    earliest_article_date = referral_send_date + 1.day # Wait until day after referral
    first_sunday = next_sunday_at_10am(earliest_article_date)

    first_sunday + week_offset.weeks
  end

  def next_sunday_at_10am(from_date)
    days_until_sunday = (7 - from_date.wday) % 7
    days_until_sunday = 7 if days_until_sunday == 0 # If already Sunday, wait for next Sunday
    (from_date + days_until_sunday.days).beginning_of_day + 10.hours
  end
end
