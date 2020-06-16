require 'rails_helper'

RSpec.describe Affiliates::UpdateBtcAddress do
  describe '#call' do
    let(:service) { described_class.new }
    let(:affiliate) { create(:affiliate) }

    subject { service.call(affiliate: affiliate, new_btc_address: new_btc_address) }

    context 'given invalid bitcoin address' do
      let (:btc_address) { Faker::Blockchain::Bitcoin.testnet_address }

      it { is_expected.to eq Result::Failure.new('Invalid bitcoin address') }
    end
  end
end
