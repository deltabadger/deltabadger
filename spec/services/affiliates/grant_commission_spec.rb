require 'rails_helper'

RSpec.describe Affiliates::GrantCommission do
  describe '#call' do
    subject { described_class.new.call(referee: referee, payment: payment) }

    let!(:referrer) { create(:user) }

    let!(:affiliate) do
      create(:affiliate,
             user: referrer,
             max_profit: max_profit,
             unexported_crypto_commission: unexported_crypto_commission,
             exported_crypto_commission: exported_crypto_commission,
             paid_crypto_commission: paid_crypto_commission)
    end

    let!(:referee) do
      create(:user,
             referrer: affiliate,
             current_referrer_profit: current_referrer_profit)
    end

    let!(:payment) do
      create(:payment,
             user: referee,
             commission: commission,
             crypto_commission: crypto_commission)
    end

    context 'when there is no commission' do
      let(:max_profit) { 20 }
      let(:unexported_crypto_commission) { 0.1 }
      let(:exported_crypto_commission) { 0.2 }
      let(:paid_crypto_commission) { 0.3 }
      let(:current_referrer_profit) { 0 }
      let(:commission) { 0 }
      let(:crypto_commission) { 0 }

      it "does not modifiy referrer's commissions" do
        subject

        referee.reload
        affiliate.reload

        expect(referee.current_referrer_profit).to eq(0)

        expect(affiliate.unexported_crypto_commission).to eq(0.1)
        expect(affiliate.exported_crypto_commission).to eq(0.2)
        expect(affiliate.paid_crypto_commission).to eq(0.3)
      end
    end

    context 'when added commission is below max profit' do
      let(:max_profit) { 20 }
      let(:unexported_crypto_commission) { 0.1 }
      let(:exported_crypto_commission) { 0.2 }
      let(:paid_crypto_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:crypto_commission) { 0.2 }

      it 'grants referrer the commission' do
        subject

        referee.reload
        affiliate.reload

        expect(referee.current_referrer_profit).to eq(8)

        expect(affiliate.unexported_crypto_commission).to eq(0.3)
        expect(affiliate.exported_crypto_commission).to eq(0.2)
        expect(affiliate.paid_crypto_commission).to eq(0.3)
      end
    end

    context 'when added commission does not fit in max profit' do
      let(:max_profit) { 7 }
      let(:unexported_crypto_commission) { 0.1 }
      let(:exported_crypto_commission) { 0.2 }
      let(:paid_crypto_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:crypto_commission) { 0.2 }

      it 'grants referrer part of the commission' do
        subject

        referee.reload
        affiliate.reload

        expect(referee.current_referrer_profit).to eq(7)

        expect(affiliate.unexported_crypto_commission).to eq(0.2)
        expect(affiliate.exported_crypto_commission).to eq(0.2)
        expect(affiliate.paid_crypto_commission).to eq(0.3)
      end
    end

    context 'when current_referrer_profit is equal to max profit' do
      let(:max_profit) { 6 }
      let(:unexported_crypto_commission) { 0.1 }
      let(:exported_crypto_commission) { 0.2 }
      let(:paid_crypto_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:crypto_commission) { 0.2 }

      it "does not modifiy referrer's commissions" do
        subject

        referee.reload
        affiliate.reload

        expect(referee.current_referrer_profit).to eq(6)

        expect(affiliate.unexported_crypto_commission).to eq(0.1)
        expect(affiliate.exported_crypto_commission).to eq(0.2)
        expect(affiliate.paid_crypto_commission).to eq(0.3)
      end
    end

    context 'when current_referrer_profit is greater than max profit' do
      let(:max_profit) { 3 }
      let(:unexported_crypto_commission) { 0.1 }
      let(:exported_crypto_commission) { 0.2 }
      let(:paid_crypto_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:crypto_commission) { 0.2 }

      it "does not modifiy referrer's commissions" do
        subject

        referee.reload
        affiliate.reload

        expect(referee.current_referrer_profit).to eq(6)

        expect(affiliate.unexported_crypto_commission).to eq(0.1)
        expect(affiliate.exported_crypto_commission).to eq(0.2)
        expect(affiliate.paid_crypto_commission).to eq(0.3)
      end
    end
  end
end
