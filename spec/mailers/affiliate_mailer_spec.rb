require 'rails_helper'

RSpec.describe AffiliateMailer, type: :mailer do
  describe '#new_btc_address_confirmation' do
    let!(:user) { create(:user) }
    let(:new_btc_address) { Faker::Blockchain::Bitcoin.address }
    let(:token) { Devise.friendly_token }
    let(:mail) do
      AffiliateMailer
        .with(user: user, new_btc_address: new_btc_address, token: token)
        .new_btc_address_confirmation
    end

    it 'renders the headers' do
      expect(mail.subject).to eq('Confirm new Bitcoin address 📒')
      expect(mail.to).to eq([user.email])
      expect(mail.from).to eq(['support@deltabadger.com'])
    end

    it 'renders the body' do
      expect(mail.body.encoded).to match(token)
      expect(mail.body.encoded).to match(new_btc_address)
    end
  end

  describe '#referral_payout_notification' do
    let!(:user) { create(:user) }
    let(:amount) { 10 } # arbitrary number
    let(:mail) do
      AffiliateMailer
        .with(user: user, amount: amount)
        .referrals_payout_notification
    end

    it 'renders the headers' do
      expect(mail.subject).to eq('It\'s a payday! 💸')
      expect(mail.to).to eq([user.email])
      expect(mail.from).to eq(['support@deltabadger.com'])
    end

    it 'renders the body' do
      expect(mail.body.encoded).to match(amount.to_s)
    end
  end
end
