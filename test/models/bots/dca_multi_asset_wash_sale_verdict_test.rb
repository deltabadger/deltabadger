require 'test_helper'

# The wash-sale verdict the metrics walk gives a sale the venue did not price. With lots behind it the
# loss is unknown, and unknown locks. With NO lots behind it — coins the bot never bought, which a
# one-asset basket may sell as the pair bot did — there is nothing of its own to judge, and nothing to
# lock: the same answer TaxLots.loss_in? gives an empty list.
class Bots::DcaMultiAssetWashSaleVerdictTest < ActiveSupport::TestCase
  def setup
    @base = create(:asset, :bitcoin)
    @bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
  end

  test 'a sale that consumed no lots and reported no proceeds carries a false verdict and locks nothing' do
    sale = unpriced_sale

    assert_equal false, @bot.metrics(force: true)[:loss_lot_by_transaction][sale.id]
    @bot.reconcile_wash_sale_from_fill!(sale)
    assert_empty @bot.user.locked_asset_ids
  end

  test 'a sale with lots behind it and no proceeds is still unknown, and still locks' do
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                         external_id: 'b-1', side: :buy, transaction_type: 'REGULAR', base: @base.symbol,
                         quote: @bot.quote_asset.symbol, price: 100, amount: 1, amount_exec: 1,
                         quote_amount: 100, quote_amount_exec: 100)
    sale = unpriced_sale

    assert_nil @bot.metrics(force: true)[:loss_lot_by_transaction][sale.id]
    @bot.reconcile_wash_sale_from_fill!(sale)
    assert_includes @bot.user.locked_asset_ids, @base.id
  end

  private

  def unpriced_sale
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                         external_id: "s-#{SecureRandom.hex(3)}", side: :sell, transaction_type: 'REGULAR',
                         base: @base.symbol, quote: @bot.quote_asset.symbol, price: 90, amount: 1, amount_exec: 1,
                         quote_amount: 90, quote_amount_exec: nil)
  end
end
