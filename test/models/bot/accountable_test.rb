require 'test_helper'

class Bot::AccountableTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # Overbuy regression: a submitted/unknown order (placed on the exchange but not
  # yet confirmed) carries quote_amount and MUST be counted as reserved spend, the
  # same as an open order. Otherwise the bot under-counts what it already committed
  # and overbuys on the next cycle while confirmation keeps failing.

  test 'pending_quote_amount reserves an open order by its ordered quote_amount' do
    bot = create(:dca_single_asset, :started) # effective_quote_amount 100, one interval
    assert_equal 100.0, bot.pending_quote_amount

    create(:transaction, bot: bot, status: :submitted, external_status: :open,
                         external_id: 'o1', quote_amount: 100, created_at: Time.current)
    bot.reload

    assert_equal 0, bot.pending_quote_amount
  end

  test 'pending_quote_amount reserves a submitted/unknown order by its ordered quote_amount' do
    bot = create(:dca_single_asset, :started)
    assert_equal 100.0, bot.pending_quote_amount

    create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                         external_id: 'u1', quote_amount: 100,
                         amount_exec: nil, quote_amount_exec: nil, created_at: Time.current)
    bot.reload

    assert_equal 0, bot.pending_quote_amount
  end
end
