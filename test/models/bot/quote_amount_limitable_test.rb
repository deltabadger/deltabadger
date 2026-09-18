require 'test_helper'

class Bot::QuoteAmountLimitableTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # A submitted/unknown order must count toward the amount-limit tally exactly like
  # an open order, otherwise the bot can spend past the user's configured limit while
  # confirmation keeps failing.

  # The cap bounds what a bot BUYS. Having spent it is the usual reason to reverse into selling, so it
  # must not be what stops the selling bot from starting — and the control is hidden while selling.
  %i[dca_single_asset dca_multi_asset].each do |type|
    test "a #{type} bot that spent its buy cap cannot start buying, but can start selling" do
      bot = create(type)
      bot.set_missed_quote_amount
      bot.update!(settings: bot.settings.merge('quote_amount_limited' => true, 'quote_amount_limit' => 500))
      bot.stubs(:quote_amount_limit_reached?).returns(true)

      bot.valid?(:start)
      assert cap_reached_error?(bot), 'a buying bot is held to its cap'

      bot.direction = 'selling'
      bot.valid?(:start)
      assert_not cap_reached_error?(bot), 'a selling bot spends nothing'
    end
  end

  test 'available-before-limit counts submitted/unknown orders as spent' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.update!(settings: bot.settings.merge('quote_amount_limited' => true, 'quote_amount_limit' => 500))
    bot.reload
    enabled_at = bot.quote_amount_limit_enabled_at
    assert_not_nil enabled_at

    create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                         external_id: 'u1', quote_amount: 200,
                         amount_exec: nil, quote_amount_exec: nil,
                         created_at: enabled_at + 1.second)
    bot.reload

    assert_equal 300, bot.quote_amount_available_before_limit_reached
  end

  private

  def cap_reached_error?(bot)
    bot.errors.details[:settings].any? { |detail| detail[:error] == :quote_amount_limit_reached }
  end
end
