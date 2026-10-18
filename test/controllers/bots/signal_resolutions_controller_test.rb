require 'test_helper'

# The user's word that they checked the venue after an ambiguous signal placement. It lifts the one
# marker the page was rendered for, and nothing else: no row is written, whatever the venue showed.
class Bots::SignalResolutionsControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @bot = create(:signal_bot, :started)
    create(:bot_signal, bot: @bot)
    @bot.update_columns(transient_data: { 'rust_placement' => { 'kept' => true } })
    @bot.mark_placement_unresolved!(side: :buy, error: 'Net::ReadTimeout')
    @marker_id = @bot.unresolved_placement['id']
    sign_in @bot.user
  end

  def resolve(marker_id = @marker_id)
    post bot_signal_resolution_path(bot_id: @bot.id), params: { marker_id: marker_id }.compact
  end

  def state
    { transient_data: @bot.reload.transient_data, logs: @bot.bot_activity_logs.pluck(:event, :details),
      transactions: Transaction.count }
  end

  test 'the matching marker is removed, the line logged, and no row written' do
    assert_no_difference 'Transaction.count' do
      resolve
    end

    assert_redirected_to bot_path(@bot)
    assert_equal I18n.t('bot.signal.resolved'), flash[:notice]
    assert_equal({ 'rust_placement' => { 'kept' => true } }, @bot.reload.transient_data)
    log = @bot.bot_activity_logs.sole
    assert_equal 'signal_placement_resolved', log.event
    assert_equal({ 'user_id' => @bot.user_id, 'marker_id' => @marker_id }, log.details)
  end

  test 'an open bot page drops the marker when it is resolved, and nothing is sent for a stale one' do
    streams = capture_turbo_stream_broadcasts(@bot.page_stream) { resolve }

    stream = streams.sole
    assert_equal %w[replace signal-unresolved-placement], [stream['action'], stream['target']]
    assert_not_includes stream.to_html, 'marker_id'

    assert_empty(capture_turbo_stream_broadcasts(@bot.page_stream) { resolve })
  end

  test 'a different marker id changes nothing' do
    before = state

    resolve(SecureRandom.uuid)

    assert_equal I18n.t('bot.signal.resolution_stale'), flash[:alert]
    assert_equal before, state
  end

  test 'a double submit: the second is a stale no-op' do
    resolve
    after_first = state

    resolve

    assert_equal I18n.t('bot.signal.resolution_stale'), flash[:alert]
    assert_equal after_first, state
  end

  # A stale tab must not lift a newer ambiguity than the one it was rendered for.
  test 'a repeat of an old submission leaves a newer marker in place' do
    resolve
    @bot.mark_placement_unresolved!(side: :sell, error: 'HTTP 504')
    newer = state

    resolve

    assert_equal I18n.t('bot.signal.resolution_stale'), flash[:alert]
    assert_equal newer, state
  end

  test 'a missing marker id changes nothing' do
    before = state

    resolve(nil)

    assert_equal I18n.t('bot.signal.resolution_stale'), flash[:alert]
    assert_equal before, state
  end

  test "another user's bot is not found" do
    sign_in create(:user)

    resolve

    assert_response :not_found
    assert_predicate @bot.reload, :placement_unresolved?
  end

  test 'requires authentication' do
    sign_out @bot.user

    resolve

    assert_redirected_to new_user_session_path
    assert_predicate @bot.reload, :placement_unresolved?
  end

  test 'the bot page shows the marker with a Resolve form naming it' do
    get bot_path(id: @bot.id)

    assert_response :ok
    assert_match I18n.t('bot.signal.unresolved', exchange: @bot.exchange.name), response.body
    assert_select "form[action='#{bot_signal_resolution_path(@bot)}'] input[name='marker_id'][value='#{@marker_id}']"
    assert_select 'form button', text: I18n.t('bot.signal.resolve', exchange: @bot.exchange.name)
  end

  test 'the bot page shows no Resolve form without a marker' do
    resolve
    get bot_path(id: @bot.id)

    assert_select "form[action='#{bot_signal_resolution_path(@bot)}']", count: 0
  end

  test 'the copy exists natively in every locale' do
    keys = %w[bot.signal.unresolved bot.signal.resolve bot.signal.resolved bot.signal.resolution_stale
              bot_activity.events.signal_placement_resolved]
    I18n.available_locales.each do |locale|
      keys.each { |key| assert I18n.exists?(key, locale, fallback: false), "#{key} missing in #{locale}" }
    end
  end
end
