# frozen_string_literal: true

require 'test_helper'

# The bot log's filter tabs. The set is fluid — a tab only exists once the bot has a row
# for it — and the control hides entirely while everything falls in one category.
#
# Three of the four tabs are named after what they hold; the fourth, "Other", is the
# catch-all for the rows with nothing to show in the Amount/Value/Price columns:
# cancelled, abandoned, skipped and failed. They appear there as their message sentence.
class OrderFiltersTest < ActionView::TestCase
  def render_filters(bot)
    render partial: 'bots/orders/order_filters', locals: { bot: bot }
  end

  def values
    css_select('.segmented__option').map { |option| option['data-value'] }
  end

  setup do
    @bot = create(:dca_single_asset)
    create(:transaction, bot: @bot, status: :submitted, external_status: :closed, external_id: 'cl1')
  end

  test 'a skipped row opens the Other tab' do
    create(:transaction, :skipped, bot: @bot)

    render_filters(@bot)

    assert_equal %w[all successful other], values
  end

  test 'a failed row opens the Other tab' do
    create(:transaction, :failed, bot: @bot)

    render_filters(@bot)

    assert_equal %w[all successful other], values
  end

  test 'a cancelled row opens the Other tab rather than a Cancelled one' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :cancelled, external_id: 'c1')

    render_filters(@bot)

    assert_equal %w[all successful other], values
  end

  test 'cancelled, skipped and failed rows share the one Other tab' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :cancelled, external_id: 'c1')
    create(:transaction, :skipped, bot: @bot)
    create(:transaction, :failed, bot: @bot)

    render_filters(@bot)

    assert_equal %w[all successful other], values
  end

  test 'Other sits after Scheduled, last, as the catch-all' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :open, external_id: 'o1')
    create(:transaction, :skipped, bot: @bot)

    render_filters(@bot)

    assert_equal %w[all successful waiting other], values
  end

  test 'the tab is labelled Other' do
    create(:transaction, :skipped, bot: @bot)

    render_filters(@bot)

    assert_select '.segmented__option[data-value=other]', text: I18n.t('order_filters.other')
  end

  test 'one category means no filters at all' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :closed, external_id: 'cl2')

    render_filters(@bot)

    assert_select '.segmented', 0
  end

  # --- balances hidden --------------------------------------------------------------------
  #
  # "All" is the sentence timeline, which states the money in prose, so it is the one tab hiding
  # balances takes away.

  test 'hiding balances drops the All tab' do
    create(:transaction, :skipped, bot: @bot)
    @bot.user.update!(hide_balances: true)

    render_filters(@bot)

    assert_equal %w[successful other], values
  end

  test 'the log opens on the first tab that still has rows' do
    create(:transaction, :skipped, bot: @bot)
    @bot.user.update!(hide_balances: true)

    render_filters(@bot)

    assert_select '.filters[data-order-filter-default=successful]'
  end

  # Activity rows lived only under All, so Other has to open for them — otherwise a bot's own
  # events would have nowhere left to show.
  test 'a bot event opens the Other tab once All is gone' do
    @bot.bot_activity_logs.create!(event: 'started')
    @bot.user.update!(hide_balances: true)

    render_filters(@bot)

    assert_equal %w[successful other], values
  end

  # ...but only an event the feed would actually show: order_skipped is already represented by its
  # own transaction row and never reaches the list.
  test 'an event the feed excludes opens nothing' do
    @bot.bot_activity_logs.create!(event: 'order_skipped')
    @bot.user.update!(hide_balances: true)

    render_filters(@bot)

    assert_select '.segmented', 0
  end

  test 'one category still means no filters at all' do
    @bot.user.update!(hide_balances: true)

    render_filters(@bot)

    assert_select '.segmented', 0
    assert_select '.filters[data-order-filter-default=successful]'
  end
end
