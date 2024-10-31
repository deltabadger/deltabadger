require 'rails_helper'

RSpec.describe Affiliates::GrantCommission do
  describe '#call' do
    subject { described_class.new.call(referral: referral, payment: payment) }

    let!(:referrer) { create(:user) }

    let!(:subscription) do
      investor = SubscriptionPlan.find_by!(name: 'investor')
      Subscription.create(
        user: referrer,
        subscription_plan: investor,
        end_time: Time.current + investor.duration + 1.day,
        credits: investor.credits
      )
    end

    let!(:affiliate) do
      create(:affiliate,
             user: referrer,
             max_profit: max_profit,
             unexported_btc_commission: unexported_btc_commission,
             exported_btc_commission: exported_btc_commission,
             paid_btc_commission: paid_btc_commission)
    end

    let!(:referral) do
      create(:user,
             referrer: affiliate,
             current_referrer_profit: current_referrer_profit)
    end

    let!(:payment) do
      create(:payment,
             user: referral,
             commission: commission,
             btc_commission: btc_commission)
    end

    context 'when referrer has no active subscription' do
      let(:max_profit) { 20 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 0 }
      let(:commission) { 2 }
      let(:btc_commission) { 0.2 }

      before do
        saver = SubscriptionPlan.find_by!(name: 'saver')
        subscription.update(subscription_plan: saver)
      end

      it "does not modifiy referrer's commissions" do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(0)

        expect(affiliate.unexported_btc_commission).to eq(0.1)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end

    context 'when referrer is inactive' do
      let(:max_profit) { 20 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 0 }
      let(:commission) { 2 }
      let(:btc_commission) { 0.2 }

      before do
        affiliate.update(active: false)
      end

      it "does not modifiy referrer's commissions" do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(0)

        expect(affiliate.unexported_btc_commission).to eq(0.1)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end

    context 'when there is no commission' do
      let(:max_profit) { 20 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 0 }
      let(:commission) { 0 }
      let(:btc_commission) { 0 }

      it "does not modifiy referrer's commissions" do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(0)

        expect(affiliate.unexported_btc_commission).to eq(0.1)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end

    context 'when added commission is below max profit' do
      let(:max_profit) { 20 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:btc_commission) { 0.2 }

      it 'grants referrer the commission' do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(8)

        expect(affiliate.unexported_btc_commission).to eq(0.3)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end

    context 'when added commission does not fit in max profit' do
      let(:max_profit) { 7 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:btc_commission) { 0.2 }

      it 'grants referrer part of the commission' do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(7)

        expect(affiliate.unexported_btc_commission).to eq(0.2)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end

    context 'when current_referrer_profit is equal to max profit' do
      let(:max_profit) { 6 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:btc_commission) { 0.2 }

      it "does not modifiy referrer's commissions" do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(6)

        expect(affiliate.unexported_btc_commission).to eq(0.1)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end

    context 'when current_referrer_profit is greater than max profit' do
      let(:max_profit) { 3 }
      let(:unexported_btc_commission) { 0.1 }
      let(:exported_btc_commission) { 0.2 }
      let(:paid_btc_commission) { 0.3 }
      let(:current_referrer_profit) { 6 }
      let(:commission) { 2 }
      let(:btc_commission) { 0.2 }

      it "does not modifiy referrer's commissions" do
        subject

        referral.reload
        affiliate.reload

        expect(referral.current_referrer_profit).to eq(6)

        expect(affiliate.unexported_btc_commission).to eq(0.1)
        expect(affiliate.exported_btc_commission).to eq(0.2)
        expect(affiliate.paid_btc_commission).to eq(0.3)
      end
    end
  end
end
