# frozen_string_literal: true

require 'test_helper'

# The modal asks about money from past sales, one row per bot, and names the assets the new bots will buy;
# the POST splits exactly the bots it was shown, or answers with the reason it refused.
class Bots::SplitsControllerTest < ActionDispatch::IntegrationTest
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    @one = basket([@btc, @eth])
    @two = basket([@eth, @sol])
    sign_in @user
  end

  # == The modal ==

  test 'the modal names the assets and posts exactly the bots it was shown' do
    get new_bots_split_path(ids: [@one.id, @two.id])

    assert_response :success
    assert_select 'turbo-frame#modal turbo-frame#split_modal'
    assert_select '.modal', text: /#{Regexp.escape(I18n.t('bot.split.explanation'))}/
    assert_equal(%w[BTC ETH SOL], css_select('.modal .ticker-group .ticker').map { it.text.strip })
    assert_select '[data-split-row]', count: 0
    ids = css_select("form[action='#{bots_split_path}'] input[name='ids[]']").map { |input| input['value'].to_i }
    assert_equal [@one.id, @two.id], ids
  end

  test 'money from past sales is a question per bot, not a refusal' do
    with_proceeds(@one)

    get new_bots_split_path(ids: [@one.id, @two.id])

    assert_select "[data-split-row=offer][data-bot-id='#{@one.id}']", count: 1
    assert_select '[data-split-row]', count: 1
    assert_select '.modal', text: /#{Regexp.escape(I18n.t('bot.split.proceeds', label: @one.label, amount: '150.00', symbol: 'USD'))}/
    assert_select "form[action='#{bot_redeploy_path(bot_id: @one.id)}'][data-split-answer]"
    assert_select "[data-action='bots-split-modal#keep'][data-bots-split-modal-id-param='#{@one.id}']"
  end

  test 'a reinvestment in flight is a pending row; an unconfirmed one is halted' do
    order!(@one, base: 'BTC', transaction_type: 'REDEPLOY', external_status: :open, amount_exec: nil, quote_amount_exec: nil)
    @two.merge_transient_data!(Bot::Composition::Redeployable::PENDING_KEY => { 'id' => 'x', 'state' => 'ambiguous' })

    get new_bots_split_path(ids: [@one.id, @two.id])

    assert_select "[data-split-row=pending][data-bot-id='#{@one.id}']" do
      assert_select 'p', text: /from past sales/, message: 'the question stays while it is answered'
      assert_select '.redeploy-prompt__actions > .loader--small', count: 1
      assert_select 'button', count: 0
    end
    assert_select "[data-split-row=halted][data-bot-id='#{@two.id}']"
  end

  test 'a selection that cannot be split gets the reason inside the frame and no Split' do
    single = basket([@btc])

    get new_bots_split_path(ids: [single.id])

    assert_select 'turbo-frame#split_modal', text: /#{Regexp.escape(I18n.t('errors.bots.split.unavailable', label: single.label))}/
    assert_select "form[action='#{bots_split_path}']", count: 0
  end

  # == Confirming ==

  test 'confirming splits, deletes the sources and sends the browser to the dashboard' do
    assert_difference 'Bots::DcaMultiAsset.not_deleted.count', 2 do
      post bots_split_path, params: { ids: [@one.id, @two.id] }, as: :turbo_stream
    end

    assert_response :success
    assert_match %(action="redirect" target="#{bots_path}"), response.body
    assert_equal I18n.t('bot.split.success'), flash[:notice]
    assert_predicate @one.reload, :deleted?
    assert_predicate @two.reload, :deleted?
  end

  test 'unanswered proceeds answer 422; kept, they split' do
    with_proceeds(@one)

    assert_no_difference 'Bot.count' do
      post bots_split_path, params: { ids: [@one.id] }, as: :turbo_stream
    end
    assert_response :unprocessable_content
    assert_includes response.body, ERB::Util.html_escape(I18n.t('errors.bots.split.proceeds', label: @one.label))

    post bots_split_path, params: { ids: [@one.id], keep_ids: [@one.id] }, as: :turbo_stream
    assert_response :success
    assert_predicate @one.reload, :deleted?
  end

  test 'a stranger id is a missing bot' do
    theirs = create(:dca_multi_asset, user: create(:user), exchange: @exchange, quote_asset: @usd,
                                      base_assets: [@btc, @eth], status: :stopped)

    post bots_split_path, params: { ids: [@one.id, theirs.id] }, as: :turbo_stream

    assert_response :unprocessable_content
    assert_includes response.body, I18n.t('errors.bots.split.missing')
    assert_not_predicate theirs.reload, :deleted?
    assert_not_predicate @one.reload, :deleted?
  end

  test 'the dashboard offers Split for a multi-asset bot and marks the tiles it can pick' do
    single = basket([@btc])

    get bots_path

    assert_select 'button.dropdown__item[data-bots-merge-mode-param=split]', text: I18n.t('button.split')
    assert_select ".bot-tile[data-bot-id='#{@one.id}'][data-split-ok=true]"
    assert_select ".bot-tile[data-bot-id='#{single.id}'][data-split-ok=false]"
  end

  private

  def basket(assets)
    create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: assets, status: :stopped)
  end

  def with_proceeds(bot)
    BotIndexAsset.create!(bot:, asset: @sol, ticker: Ticker.find_by(exchange: @exchange, base_asset: @sol, quote_asset: @usd),
                          in_index: false, target_allocation: 0, entered_at: 1.week.ago, exited_at: 1.day.ago)
    order!(bot, base: 'SOL', created_at: 3.days.ago)
    order!(bot, base: 'SOL', side: :sell, transaction_type: 'LIQUIDATION', created_at: 2.days.ago,
                price: 150, quote_amount: 150, quote_amount_exec: 150)
  end

  def order!(bot, base:, **columns)
    create(:transaction, exchange: @exchange, bot:, base:, quote: 'USD', status: :submitted, external_status: :closed,
                         side: :buy, external_id: "o-#{SecureRandom.hex(4)}", price: 100, amount: 1, amount_exec: 1,
                         quote_amount: 100, quote_amount_exec: 100, **columns)
  end
end
