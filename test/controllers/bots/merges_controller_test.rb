# frozen_string_literal: true

require 'test_helper'

# The confirmation modal names exactly what the merge will do and freezes the chosen bots into the
# request; the POST answers with the new bot's page or with the reason it refused.
class Bots::MergesControllerTest < ActionDispatch::IntegrationTest
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    @anchor = basket([@btc])
    @anchor.update_columns(settings: @anchor.settings.merge('interval' => 'week', 'quote_amount' => 250))
    @other = basket([@eth, @sol])
    sign_in @user
  end

  # == The modal ==

  test 'the modal says what will happen and posts exactly the bots it was shown' do
    get new_bots_merge_path(ids: [@anchor.id, @other.id])

    assert_response :success
    assert_select 'turbo-frame#modal'
    assert_select '.modal .modal__title', text: I18n.t('bot.merge.title', count: 2)
    assert_select '.modal', text: /investing USD on Binance/
    assert_select '.modal select[name=exchange_id]', count: 0, message: 'one venue: nothing to pick'
    assert_select '.modal', text: /250/, count: 0
    assert_select '.modal .ticker-group.ticker-group--many.ticker-group--fold[data-controller="ticker-fold"] ' \
                  '.ticker[data-ticker-asset-id]', count: 3
    assert_equal(%w[BTC ETH SOL], css_select('.modal .ticker-group .ticker').map { it.text.strip })
    assert_select '.modal', text: /#{Regexp.escape(I18n.t('bot.merge.deleted_note'))}/
    assert_select "form[action='#{bots_merge_path}'] input[type=hidden][name='ids[]']", count: 2
    ids = css_select("form[action='#{bots_merge_path}'] input[name='ids[]']").map { |input| input['value'].to_i }
    assert_equal [@anchor.id, @other.id], ids
    assert_select "form[action='#{bots_merge_path}'][data-bots-merge-form]"
    assert_select "form[action='#{bots_merge_path}'] button", text: I18n.t('button.confirm')
  end

  # The browser folds the stack into "+N" only when it runs out of width; the server sends them all.
  test 'the modal shows every asset, however many' do
    others = Array.new(6) { |i| create(:asset, symbol: "A#{i}", name: "Asset #{i}", external_id: "asset-#{i}") }
    big = basket(others)

    get new_bots_merge_path(ids: [@anchor.id, big.id])

    assert_response :success
    assert_equal(%w[BTC] + others.map(&:symbol), css_select('.modal .ticker-group .ticker').map { it.text.strip })
    assert_select '.modal .ticker--more', count: 0
  end

  test 'a selection that cannot be merged gets the reason and no Confirm' do
    eur = create(:asset, :eur)
    other_quote = create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: eur, base_assets: [@sol],
                                           status: :stopped)

    get new_bots_merge_path(ids: [@anchor.id, other_quote.id])

    assert_response :success
    assert_select '.modal', text: /#{Regexp.escape(I18n.t('errors.bots.merge.quote'))}/
    assert_select "form[action='#{bots_merge_path}']", count: 0
    assert_select '.modal button', text: I18n.t('button.close')
  end

  # A venue without a key needs no warning: the bot's own page asks for the key, and offers the venue again.
  test 'a merge that lands on a venue without a key says only that assets stay where they were bought' do
    kraken = create(:kraken_exchange)
    dot = create(:asset, symbol: 'DOT', name: 'Polkadot', external_id: 'polkadot')
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [dot],
                                         status: :stopped, with_api_key: false)
    create(:ticker, exchange: kraken, base_asset: @btc, quote_asset: @usd) # Kraken lists both; Binance lacks DOT

    get new_bots_merge_path(ids: [@anchor.id, on_kraken.id])

    assert_response :success
    assert_select '.modal', text: /on Kraken into/
    assert_equal(%w[BTC DOT], css_select('.modal .ticker-group .ticker').map { it.text.strip })
    assert_select '.modal', text: /API key/, count: 0
    assert_select '.modal', text: /#{Regexp.escape(I18n.t('bot.merge.assets_stay'))}/
    assert_select "form[action='#{bots_merge_path}'] button", text: I18n.t('button.confirm')
  end

  # The pill re-requests the modal with the pick; only the modal's body is swapped (its own frame), so
  # the dialog does not open again.
  test 'where several venues list everything, the modal offers them and posts the one it shows' do
    kraken = create(:kraken_exchange)
    [@btc, @eth, @sol].each { |asset| create(:ticker, exchange: kraken, base_asset: asset, quote_asset: @usd) }

    get new_bots_merge_path(ids: [@anchor.id, @other.id])

    assert_response :success
    assert_select "turbo-frame#merge_modal form[action='#{new_bots_merge_path}'][method=get]" \
                  '[data-turbo-frame=merge_modal][data-controller=form--submit]' do
      assert_select "input[type=hidden][name='ids[]']", count: 2
      assert_select ".sinput--select select[name=exchange_id][data-action='change->form--submit#submit'] option",
                    count: 2
      assert_select "select[name=exchange_id] option[selected][value='#{@exchange.id}']", text: 'Binance'
    end
    assert_select "form[action='#{bots_merge_path}'] input[type=hidden][name=exchange_id][value='#{@exchange.id}']"

    get new_bots_merge_path(ids: [@anchor.id, @other.id], exchange_id: kraken.id)

    assert_select "select[name=exchange_id] option[selected][value='#{kraken.id}']", text: 'Kraken'
    assert_select "form[action='#{bots_merge_path}'] input[type=hidden][name=exchange_id][value='#{kraken.id}']"
  end

  # A pick that no longer holds renders its refusal inside the swapped frame; without the frame Turbo
  # would show "Content missing" instead.
  test 'a picked venue that stopped listing everything refuses inside the same frame, with no Confirm' do
    coinbase = create(:coinbase_exchange)
    create(:ticker, exchange: coinbase, base_asset: @btc, quote_asset: @usd)

    get new_bots_merge_path(ids: [@anchor.id, @other.id], exchange_id: coinbase.id)

    assert_response :success
    assert_select 'turbo-frame#merge_modal', text: /#{Regexp.escape(I18n.t('errors.bots.merge.no_common_exchange'))}/
    assert_select "form[action='#{bots_merge_path}']", count: 0
  end

  # == The merge ==

  test 'confirming puts the new bot on the venue the modal showed' do
    kraken = create(:kraken_exchange)
    [@btc, @eth, @sol].each { |asset| create(:ticker, exchange: kraken, base_asset: asset, quote_asset: @usd) }

    post bots_merge_path, params: { ids: [@anchor.id, @other.id], exchange_id: kraken.id }, as: :turbo_stream

    assert_response :success
    assert_equal kraken, @user.bots.not_deleted.sole.exchange
  end

  test 'a confirmed venue that no longer lists everything refuses rather than landing elsewhere' do
    coinbase = create(:coinbase_exchange)
    create(:ticker, exchange: coinbase, base_asset: @btc, quote_asset: @usd)

    assert_no_difference 'Bot.count' do
      post bots_merge_path, params: { ids: [@anchor.id, @other.id], exchange_id: coinbase.id }, as: :turbo_stream
    end

    assert_response :unprocessable_content
    assert_includes response.body, I18n.t('errors.bots.merge.no_common_exchange')
    assert_predicate @anchor.reload, :stopped?
  end

  test 'confirming creates the bot, deletes the sources and sends the browser to the new page' do
    assert_difference 'Bots::DcaMultiAsset.count', 1 do
      post bots_merge_path, params: { ids: [@anchor.id, @other.id] }, as: :turbo_stream
    end

    assert_response :success
    merged = @user.bots.not_deleted.sole
    assert_match %(action="redirect" target="#{bot_path(merged)}"), response.body
    assert_equal I18n.t('bot.merge.success'), flash[:notice]
    assert_equal 'BTC, ETH, SOL', merged.label
    assert_predicate @anchor.reload, :deleted?
    assert_predicate @other.reload, :deleted?
  end

  test 'a refused merge answers 422 with the reason and creates nothing' do
    signal = create(:signal_bot, user: @user, exchange: @exchange, base_asset: @sol, quote_asset: @usd)

    assert_no_difference 'Bot.count' do
      post bots_merge_path, params: { ids: [@anchor.id, signal.id] }, as: :turbo_stream
    end

    assert_response :unprocessable_content
    assert_match(/turbo-stream action="prepend" target="flash"/, response.body)
    assert_includes response.body, ERB::Util.html_escape(I18n.t('errors.bots.merge.unavailable', label: signal.label))
    assert_predicate @anchor.reload, :stopped?
  end

  test 'a stranger id is a missing bot, not a shorter merge' do
    theirs = create(:dca_multi_asset, user: create(:user), exchange: @exchange, quote_asset: @usd, base_assets: [@sol],
                                      status: :stopped)

    assert_no_difference 'Bot.count' do
      post bots_merge_path, params: { ids: [@anchor.id, @other.id, theirs.id] }, as: :turbo_stream
    end

    assert_response :unprocessable_content
    assert_includes response.body, I18n.t('errors.bots.merge.missing')
    assert_predicate theirs.reload, :stopped?
    assert_predicate @other.reload, :stopped?
  end

  test 'signed out, both actions go to sign-in' do
    sign_out @user

    get new_bots_merge_path(ids: [@anchor.id, @other.id])
    assert_redirected_to new_user_session_path

    post bots_merge_path, params: { ids: [@anchor.id, @other.id] }
    assert_redirected_to new_user_session_path
    assert_predicate @anchor.reload, :stopped?
  end

  private

  def basket(assets)
    create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: assets, status: :stopped)
  end
end
