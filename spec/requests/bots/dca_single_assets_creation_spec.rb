require 'rails_helper'

RSpec.describe "DCA Single Asset Bot Creation", type: :request do
  # Admin user must exist to bypass setup redirect
  let!(:admin) { create(:user, admin: true) }
  let(:user) { create(:user) }
  let(:exchange) { create(:binance_exchange) }
  let(:bitcoin) { create(:asset, :bitcoin) }
  let(:usd) { create(:asset, :usd) }
  let!(:ticker) { create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd) }
  let!(:api_key) { create(:api_key, user: user, exchange: exchange, key_type: :trading, status: :correct) }

  before do
    sign_in user
    allow(Bot::ActionJob).to receive(:perform_later)
  end

  describe "complete wizard flow" do
    it "creates a bot when completing all steps" do
      # Step 1: Pick buyable asset
      get new_bots_dca_single_assets_pick_buyable_asset_path
      expect(response).to have_http_status(:ok)

      post bots_dca_single_assets_pick_buyable_asset_path, params: {
        bots_dca_single_asset: { base_asset_id: bitcoin.id }
      }
      expect(response).to redirect_to(new_bots_dca_single_assets_pick_exchange_path)
      follow_redirect!

      # Step 2: Pick exchange
      expect(response).to have_http_status(:ok)

      post bots_dca_single_assets_pick_exchange_path, params: {
        bots_dca_single_asset: { exchange_id: exchange.id }
      }
      expect(response).to redirect_to(new_bots_dca_single_assets_add_api_key_path)
      follow_redirect!

      # Step 3: API key (already validated, should redirect)
      expect(response).to redirect_to(new_bots_dca_single_assets_pick_spendable_asset_path)
      follow_redirect!

      # Step 4: Pick spendable asset
      expect(response).to have_http_status(:ok)

      post bots_dca_single_assets_pick_spendable_asset_path, params: {
        bots_dca_single_asset: { quote_asset_id: usd.id }
      }
      expect(response).to redirect_to(new_bots_dca_single_assets_confirm_settings_path)
      follow_redirect!

      # Step 5: Confirm settings
      expect(response).to have_http_status(:ok)

      post bots_dca_single_assets_confirm_settings_path, params: {
        bots_dca_single_asset: { quote_amount: 100, interval: 'day' }
      }, as: :turbo_stream
      expect(response).to have_http_status(:ok)

      # Step 6: Create bot
      expect {
        post bots_dca_single_assets_path, as: :turbo_stream
      }.to change(Bots::DcaSingleAsset, :count).by(1)

      bot = Bots::DcaSingleAsset.last
      expect(bot.base_asset).to eq(bitcoin)
      expect(bot.quote_asset).to eq(usd)
      expect(bot.exchange).to eq(exchange)
      expect(bot.quote_amount).to eq(100)
      expect(bot).to be_scheduled
    end

    it "creates bot with weekly interval" do
      get new_bots_dca_single_assets_pick_buyable_asset_path

      post bots_dca_single_assets_pick_buyable_asset_path, params: {
        bots_dca_single_asset: { base_asset_id: bitcoin.id }
      }
      follow_redirect!

      post bots_dca_single_assets_pick_exchange_path, params: {
        bots_dca_single_asset: { exchange_id: exchange.id }
      }
      follow_redirect!
      follow_redirect! # skip API key

      post bots_dca_single_assets_pick_spendable_asset_path, params: {
        bots_dca_single_asset: { quote_asset_id: usd.id }
      }
      follow_redirect!

      post bots_dca_single_assets_confirm_settings_path, params: {
        bots_dca_single_asset: { quote_amount: 50, interval: 'week' }
      }, as: :turbo_stream

      expect {
        post bots_dca_single_assets_path, as: :turbo_stream
      }.to change(Bots::DcaSingleAsset, :count).by(1)

      bot = Bots::DcaSingleAsset.last
      expect(bot.quote_amount).to eq(50)
      expect(bot.interval).to eq('week')
    end
  end

  describe "wizard navigation guards" do
    it "redirects to pick asset when accessing exchange step directly" do
      get new_bots_dca_single_assets_pick_exchange_path
      expect(response).to redirect_to(new_bots_dca_single_assets_pick_buyable_asset_path)
    end
  end

  describe "authentication" do
    before { sign_out user }

    it "requires authentication for wizard" do
      get new_bots_dca_single_assets_pick_buyable_asset_path
      expect(response).to redirect_to(new_user_session_path)
    end

    it "requires authentication to create bot" do
      post bots_dca_single_assets_path, as: :turbo_stream
      expect(response).to redirect_to(new_user_session_path)
    end
  end
end
