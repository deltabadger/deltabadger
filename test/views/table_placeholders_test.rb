# frozen_string_literal: true

require 'test_helper'

# Two things in a table are not the table's content, and neither is set in its ink: a dash standing
# where a value would be, and the date a row is filed under. Both are decided in one place — the
# `no_value` helper and the `table__when` class — so every table on the bot and tracker pages says
# it the same way.
class TablePlaceholdersTest < ActionView::TestCase
  setup do
    @bot = create(:dca_single_asset)
    # These rows are rendered for a signed-in user, and a view test has no request carrying one —
    # Devise's own `current_user` helper is on the view, so the stand-in has to be too.
    user = @bot.user
    view.define_singleton_method(:current_user) { user }
  end

  test 'a cell with nothing to show carries the muted dash' do
    render partial: 'bots/assets_table',
           locals: { rows: [{ symbol: 'BTC', amount: '1', avg_price: nil, value: nil, pnl: nil }] }

    # Average price, value and P/L — every column that has nothing to state.
    assert_select 'td span.no-value', 3
  end

  test 'the date opening a row is marked as the caption it is' do
    activity = @bot.bot_activity_logs.create!(event: 'started')

    render partial: 'bots/orders/activity', locals: { activity: activity }

    # A bare <tr> is not a document, and the fragment parser drops the row — so the markup is
    # searched rather than a parsed tree.
    assert_includes rendered, 'class="table__when"'
    # One shape on both pages: year-first date, clock time behind it in <small>.
    assert_match %r{\d{4}/\d{2}/\d{2} <small>\d{2}:\d{2}</small>}, rendered
  end
end
