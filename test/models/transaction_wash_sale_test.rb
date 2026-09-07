require 'test_helper'

class TransactionWashSaleTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    @ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    @bia = BotIndexAsset.create!(bot: @bot, asset: asset, ticker: @ticker, target_allocation: 1.0, in_index: true, entered_at: Time.current)
    @bot.set_missed_quote_amount
    @bot.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    # Transaction belongs_to :exchange (non-nullable column): every row here names the bot's venue.
    @order = @bot.transactions.create!(exchange: @bot.exchange, base: 'AAA', quote: @bot.quote_asset.symbol, side: :sell,
                                       transaction_type: 'LIQUIDATION', status: :submitted, external_status: :open,
                                       external_id: 'o1', amount: 1, price: 90, order_type: :market_order)
  end

  # The walk's verdict for this sale: `pnl` is the net (display), `loss_lot` whether any consumed
  # lot lost — the guard reads the second.
  def realised(pnl, loss_lot: pnl.to_d.negative?)
    Bots::DcaIndex.any_instance.stubs(:metrics).returns(asset_breakdown: {}, tax_pnl_by_transaction: { @order.id => pnl.to_d },
                                                        loss_lot_by_transaction: { @order.id => loss_lot })
  end

  def venue_says(status, amount_exec:, quote_amount_exec:)
    { status: status, amount_exec: amount_exec, quote_amount_exec: quote_amount_exec, price: 90, amount: 1, side: :sell, order_type: :market_order }
  end

  test 'a sell that filled at a loss the next day gets its lock extended from the day the fill was seen' do
    travel_to(Time.zone.parse('2026-09-07 15:59')) { @bot.lock_buying!(@bia.asset_id, ticker: @ticker) }
    travel_to Time.zone.parse('2026-09-08 09:31') do
      realised(-10)
      @order.update_with_order_data(venue_says(:closed, amount_exec: 1, quote_amount_exec: 90))
      assert_equal Time.zone.parse('2026-10-09 00:00'), @bia.reload.buy_locked_until
    end
  end

  test 'a closed sell that reported no executed quantity still counts: it executed what it asked for' do
    travel_to(Time.zone.parse('2026-09-07 15:59')) { @bot.lock_buying!(@bia.asset_id, ticker: @ticker) }
    travel_to Time.zone.parse('2026-09-08 09:31') do
      realised(-10)
      @order.update_with_order_data(venue_says(:closed, amount_exec: nil, quote_amount_exec: 90))
      assert_equal Time.zone.parse('2026-10-09 00:00'), @bia.reload.buy_locked_until
    end
  end

  test 'a sell estimated as a gain that filled at a loss gets a lock after all' do
    travel_to Time.zone.parse('2026-09-07 15:59') do
      realised(-1)
      @order.update_with_order_data(venue_says(:closed, amount_exec: 1, quote_amount_exec: 99))
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until
      assert @bot.bot_activity_logs.find_by(event: 'wash_sale_locked')
    end
  end

  test 'a cancelled sell that partly filled at a loss counts as a sale' do
    travel_to Time.zone.parse('2026-09-07 15:59') do
      realised(-3)
      @order.update_with_order_data(venue_says(:cancelled, amount_exec: '0.4', quote_amount_exec: 36))
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until
    end
  end

  test 'a cancelled sell with nothing executed is not a sale' do
    realised(0)
    @order.update_with_order_data(venue_says(:cancelled, amount_exec: 0, quote_amount_exec: 0))
    assert_nil @bia.reload.buy_locked_until
  end

  test 'a sale that nets a gain but lost on one lot is locked' do
    travel_to Time.zone.parse('2026-09-07 15:59') do
      realised(20, loss_lot: true)
      @order.update_with_order_data(venue_says(:closed, amount_exec: 2, quote_amount_exec: 220))
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until
    end
  end

  test 'a cancelled partial whose proceeds are unknown is locked on the safe side, and a late detail changes nothing' do
    travel_to Time.zone.parse('2026-09-07 15:59') do
      # The walk skips a sell it cannot price, so the verdict hash has no entry for this order.
      Bots::DcaIndex.any_instance.stubs(:metrics).returns(asset_breakdown: {}, loss_lot_by_transaction: {})
      @order.update_with_order_data(venue_says(:cancelled, amount_exec: '0.4', quote_amount_exec: nil))
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until, 'something was sold and nothing will re-poll it'
    end
    travel_to Time.zone.parse('2026-09-09 15:59') do
      realised(3, loss_lot: false) # the proceeds turn up two days later and say it was a gain
      @order.reload.update_with_order_data(venue_says(:cancelled, amount_exec: '0.4', quote_amount_exec: 40))
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until, 'never shortened'
    end
  end

  test 're-polling an already closed sale changes nothing' do
    travel_to Time.zone.parse('2026-09-07 15:59') do
      realised(-10)
      @order.update_with_order_data(venue_says(:closed, amount_exec: 1, quote_amount_exec: 90))
    end
    deadline = @bia.reload.buy_locked_until
    travel_to Time.zone.parse('2026-09-20 15:59') do
      Bots::DcaIndex.any_instance.expects(:metrics).never
      @order.reload.update_with_order_data(venue_says(:closed, amount_exec: 1, quote_amount_exec: 90))
      assert_equal deadline, @bia.reload.buy_locked_until
    end
  end

  test 'a sell that filled at a gain never shortens or removes a lock' do
    travel_to(Time.zone.parse('2026-09-07 15:59')) { @bot.lock_buying!(@bia.asset_id, ticker: @ticker) }
    deadline = @bia.reload.buy_locked_until
    realised(5)
    @order.update_with_order_data(venue_says(:closed, amount_exec: 1, quote_amount_exec: 105))
    assert_equal deadline, @bia.reload.buy_locked_until
  end

  test 'a buy fill does nothing here' do
    buy = @bot.transactions.create!(exchange: @bot.exchange, base: 'AAA', quote: @bot.quote_asset.symbol, side: :buy,
                                    transaction_type: 'REGULAR', status: :submitted, external_status: :open,
                                    external_id: 'o2', amount: 1, price: 90, order_type: :market_order)
    Bots::DcaIndex.any_instance.expects(:metrics).never
    buy.update_with_order_data(venue_says(:closed, amount_exec: 1, quote_amount_exec: 90).merge(side: :buy))
    assert_nil @bia.reload.buy_locked_until
  end

  test 'the bulk sweep reaches the same hook' do
    travel_to Time.zone.parse('2026-09-07 15:59') do
      realised(-10)
      @bot.stubs(:get_orders).returns(Result::Success.new(
                                        orders: { 'o1' => venue_says(:closed, amount_exec: 1, quote_amount_exec: 90).merge(ticker: @ticker) },
                                        missing: []
                                      ))
      Bot::FetchAndUpdateOpenOrdersJob.new.perform(@bot)
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until
    end
  end
end
