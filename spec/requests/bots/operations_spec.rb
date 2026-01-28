require 'rails_helper'

RSpec.describe "Bot Operations", type: :request do
  let!(:admin) { create(:user, admin: true) }
  let(:user) { create(:user) }
  let(:exchange) { create(:binance_exchange) }
  let(:bitcoin) { create(:asset, :bitcoin) }
  let(:usd) { create(:asset, :usd) }
  let!(:ticker) { create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd) }
  let!(:api_key) { create(:api_key, user: user, exchange: exchange, key_type: :trading, status: :correct) }
  let!(:bot) do
    create(:dca_single_asset,
      user: user,
      exchange: exchange,
      base_asset: bitcoin,
      quote_asset: usd,
      status: :stopped,
      with_api_key: false
    )
  end

  before do
    sign_in user
    allow(Bot::ActionJob).to receive(:perform_later)
    allow(Bot::ActionJob).to receive(:set).and_return(double(perform_later: true))
    allow(Bot::BroadcastAfterScheduledActionJob).to receive(:perform_later)
    # Skip the missed_quote_amount validation which requires specific setup
    allow_any_instance_of(Bots::DcaSingleAsset).to receive(:check_missed_quote_amount_was_set).and_return(true)
  end

  describe "starting a bot" do
    it "starts a stopped bot" do
      patch bot_start_path(bot_id: bot.id), params: { start_fresh: "true" }, as: :turbo_stream

      expect(response).to have_http_status(:ok)
      expect(bot.reload).to be_scheduled
      expect(bot.started_at).to be_present
    end

    it "starts fresh resets missed amount" do
      bot.update!(missed_quote_amount: 50)

      patch bot_start_path(bot_id: bot.id), params: { start_fresh: "true" }, as: :turbo_stream

      expect(response).to have_http_status(:ok)
      expect(bot.reload.missed_quote_amount).to eq(0)
    end

    it "schedules the action job" do
      expect(Bot::ActionJob).to receive(:perform_later).with(bot)

      patch bot_start_path(bot_id: bot.id), params: { start_fresh: "true" }, as: :turbo_stream
    end

    it "returns error when ticker unavailable" do
      ticker.update!(available: false)

      patch bot_start_path(bot_id: bot.id), params: { start_fresh: "true" }, as: :turbo_stream

      expect(response).to have_http_status(:unprocessable_content)
      expect(bot.reload).to be_stopped
    end
  end

  describe "stopping a bot" do
    before do
      bot.update!(status: :scheduled, started_at: Time.current)
    end

    it "stops a running bot" do
      patch bot_stop_path(bot_id: bot.id), as: :turbo_stream

      expect(response).to have_http_status(:ok)
      expect(bot.reload).to be_stopped
      expect(bot.stopped_at).to be_present
    end
  end

  describe "deleting a bot" do
    it "soft-deletes a bot" do
      delete bot_delete_path(bot_id: bot.id), as: :turbo_stream

      expect(response).to have_http_status(:ok)
      expect(bot.reload).to be_deleted
    end
  end

  describe "authorization" do
    let(:other_user) { create(:user) }
    let!(:other_api_key) { create(:api_key, user: other_user, exchange: exchange, key_type: :trading, status: :correct) }
    let!(:other_bot) do
      create(:dca_single_asset,
        user: other_user,
        exchange: exchange,
        base_asset: bitcoin,
        quote_asset: usd,
        with_api_key: false
      )
    end

    it "prevents accessing another user's bot" do
      patch bot_start_path(bot_id: other_bot.id), as: :turbo_stream

      expect(response).to redirect_to(bots_path)
    end

    it "prevents deleting another user's bot" do
      delete bot_delete_path(bot_id: other_bot.id), as: :turbo_stream

      expect(response).to redirect_to(bots_path)
      expect(other_bot.reload).not_to be_deleted
    end
  end

  describe "authentication" do
    before { sign_out user }

    it "requires login to start bot" do
      patch bot_start_path(bot_id: bot.id), as: :turbo_stream
      expect(response).to redirect_to(new_user_session_path)
    end

    it "requires login to delete bot" do
      delete bot_delete_path(bot_id: bot.id), as: :turbo_stream
      expect(response).to redirect_to(new_user_session_path)
    end
  end
end
