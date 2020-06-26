require 'rails_helper'

RSpec.describe Affiliates::UpdateBtcAddress do
  describe '#call' do
    let(:service) { described_class.new(affiliate_mailer: affiliate_mailer) }
    let(:affiliate_mailer) { double(:affiliate_mailer) }

    let!(:affiliate) { create(:affiliate) }


    subject { service.call(affiliate: affiliate, new_btc_address: new_btc_address) }

    context 'given invalid bitcoin address' do
      let(:new_btc_address) { 'invalid' }

      it { is_expected.to eq Result::Failure.new('Invalid bitcoin address') }
    end

    context 'given testnet bitcoin address' do
      let(:new_btc_address) { Faker::Blockchain::Bitcoin.testnet_address }

      it { is_expected.to eq Result::Failure.new('Invalid bitcoin address') }
    end

    context 'given valid bitcoin address' do
      let(:new_btc_address) { Faker::Blockchain::Bitcoin.address  }

      it { is_expected.to eq Result::Success.new }

      it 'sends email' do
        # expect(affiliate_mailer).to receive(:with).with(user: affiliate.user, new_btc_address: new_btc_address)
        expect(affiliate_mailer).to receive(:with).with(any_args())

        subject
      end
    end
  end
end
