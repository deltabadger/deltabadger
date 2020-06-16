require 'rails_helper'

RSpec.describe AffiliateMailer, type: :mailer do
  describe '#new_btc_address_confirmation' do
    let(:user) { create(:user) }
    let(:token) { 'secret_token' }
    let(:mail) { AffiliateMailer.with(user: user, token: token).new_btc_address_confirmation }

    it 'renders the headers' do
      expect(mail.subject).to eq('Confirm bitcoin address update')
      expect(mail.to).to eq([user.email])
      expect(mail.from).to eq(['support@deltabadger.com'])
    end

    it 'renders the body' do
      expect(mail.body.encoded).to match(token)
    end
  end
end
