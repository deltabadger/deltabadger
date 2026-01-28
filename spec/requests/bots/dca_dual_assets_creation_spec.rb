require 'rails_helper'

RSpec.describe "DCA Dual Asset Bot Creation", type: :request do
  # Admin user must exist to bypass setup redirect
  let!(:admin) { create(:user, admin: true) }
  let(:user) { create(:user) }
  let(:exchange) { create(:binance_exchange) }
  let(:bitcoin) { create(:asset, :bitcoin) }
  let(:ethereum) { create(:asset, :ethereum) }
  let(:usd) { create(:asset, :usd) }
  let!(:btc_ticker) { create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd) }
  let!(:eth_ticker) { create(:ticker, exchange: exchange, base_asset: ethereum, quote_asset: usd) }
  let!(:api_key) { create(:api_key, user: user, exchange: exchange, key_type: :trading, status: :correct) }

  before do
    sign_in user
    allow(Bot::ActionJob).to receive(:perform_later)
  end

  describe "complete wizard flow" do
    it "creates a bot when completing all steps" do
      # Step 1: Pick first asset
      get new_bots_dca_dual_assets_pick_first_buyable_asset_path
      expect(response).to have_http_status(:ok)

      post bots_dca_dual_assets_pick_first_buyable_asset_path, params: {
        bots_dca_dual_asset: { base0_asset_id: bitcoin.id }
      }
      expect(response).to redirect_to(new_bots_dca_dual_assets_pick_second_buyable_asset_path)
      follow_redirect!

      # Step 2: Pick second asset
      expect(response).to have_http_status(:ok)

      post bots_dca_dual_assets_pick_second_buyable_asset_path, params: {
        bots_dca_dual_asset: { base1_asset_id: ethereum.id }
      }
      expect(response).to redirect_to(new_bots_dca_dual_assets_pick_exchange_path)
      follow_redirect!

      # Step 3: Pick exchange
      expect(response).to have_http_status(:ok)

      post bots_dca_dual_assets_pick_exchange_path, params: {
        bots_dca_dual_asset: { exchange_id: exchange.id }
      }
      expect(response).to redirect_to(new_bots_dca_dual_assets_add_api_key_path)
      follow_redirect!

      # Step 4: API key (already validated, should redirect)
      expect(response).to redirect_to(new_bots_dca_dual_assets_pick_spendable_asset_path)
      follow_redirect!

      # Step 5: Pick spendable asset
      expect(response).to have_http_status(:ok)

      post bots_dca_dual_assets_pick_spendable_asset_path, params: {
        bots_dca_dual_asset: { quote_asset_id: usd.id }
      }
      expect(response).to redirect_to(new_bots_dca_dual_assets_confirm_settings_path)
      follow_redirect!

      # Step 6: Confirm settings
      expect(response).to have_http_status(:ok)

      post bots_dca_dual_assets_confirm_settings_path, params: {
        bots_dca_dual_asset: { quote_amount: 200, interval: 'day', allocation0: 0.6 }
      }, as: :turbo_stream
      expect(response).to have_http_status(:ok)

      # Step 7: Create bot
      expect {
        post bots_dca_dual_assets_path, as: :turbo_stream
      }.to change(Bots::DcaDualAsset, :count).by(1)

      bot = Bots::DcaDualAsset.last
      expect(bot.base0_asset).to eq(bitcoin)
      expect(bot.base1_asset).to eq(ethereum)
      expect(bot.quote_asset).to eq(usd)
      expect(bot.exchange).to eq(exchange)
      expect(bot.quote_amount).to eq(200)
      expect(bot.allocation0).to eq(0.6)
      expect(bot).to be_scheduled
    end

    it "creates bot with custom allocation" do
      get new_bots_dca_dual_assets_pick_first_buyable_asset_path

      post bots_dca_dual_assets_pick_first_buyable_asset_path, params: {
        bots_dca_dual_asset: { base0_asset_id: bitcoin.id }
      }
      follow_redirect!

      post bots_dca_dual_assets_pick_second_buyable_asset_path, params: {
        bots_dca_dual_asset: { base1_asset_id: ethereum.id }
      }
      follow_redirect!

      post bots_dca_dual_assets_pick_exchange_path, params: {
        bots_dca_dual_asset: { exchange_id: exchange.id }
      }
      follow_redirect!
      follow_redirect! # skip API key

      post bots_dca_dual_assets_pick_spendable_asset_path, params: {
        bots_dca_dual_asset: { quote_asset_id: usd.id }
      }
      follow_redirect!

      post bots_dca_dual_assets_confirm_settings_path, params: {
        bots_dca_dual_asset: { quote_amount: 100, interval: 'week', allocation0: 0.8 }
      }, as: :turbo_stream

      expect {
        post bots_dca_dual_assets_path, as: :turbo_stream
      }.to change(Bots::DcaDualAsset, :count).by(1)

      bot = Bots::DcaDualAsset.last
      expect(bot.allocation0).to eq(0.8)
      expect(bot.interval).to eq('week')
    end
  end

  describe "wizard navigation guards" do
    it "redirects to first asset when accessing second asset step directly" do
      get new_bots_dca_dual_assets_pick_second_buyable_asset_path
      expect(response).to redirect_to(new_bots_dca_dual_assets_pick_first_buyable_asset_path)
    end
  end

  describe "authentication" do
    before { sign_out user }

    it "requires authentication for wizard" do
      get new_bots_dca_dual_assets_pick_first_buyable_asset_path
      expect(response).to redirect_to(new_user_session_path)
    end
  end
end
