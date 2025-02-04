# Preview all emails at http://localhost:3000/rails/mailers/affiliate_mailer
class AffiliateMailerPreview < ActionMailer::Preview
  def new_btc_address_confirmation
    user = User.new(email: 'test@example.com', name: 'Mathias')
    AffiliateMailer.with(
      user: user,
      new_btc_address: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa',
      token: 'abc123'
    ).new_btc_address_confirmation
  end

  def referrals_payout_notification
    user = User.new(email: 'test@example.com', name: 'Mathias')
    AffiliateMailer.with(
      user: user,
      amount: 100
    ).referrals_payout_notification
  end

  def registration_reminder
    user = User.new(email: 'test@example.com', name: 'Mathias')
    referrer = Affiliate.new
    referrer.user = user

    AffiliateMailer.with(
      referrer: referrer,
      amount: 50
    ).registration_reminder
  end
end
