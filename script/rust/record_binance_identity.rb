# A12 PR 1 oracle: synthetic answers, unchanged Rails mappers/store and accounting engines.
# Run with disposable database URLs; never against an existing install.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers
ActiveJob::Base.queue_adapter = :test
Net::HTTP.prepend(Module.new { def connect = raise('Unexpected network in A12 recorder') })
travel_to(Time.utc(2026, 8, 18))

class A12Client
  attr_accessor :answers, :mapper_only
  def method_missing(name, **query)
    # Binance rejects Rails' flexible request, which has no type. Record separately from mapper evidence.
    return Result::Failure.new('mandatory type missing') if name == :simple_earn_flexible_rewards && !query[:type] && !mapper_only
    data = answers.fetch(name, nil)
    return Result::Failure.new('no synthetic answer') unless data
    Result::Success.new(data)
  end
  def respond_to_missing?(*); true; end
end
client = A12Client.new
Honeymaker.define_singleton_method(:client) { |*args, **opts| client }
user = User.new(email: 'a12@example.test', password: 'Synthetic-password-123!'); user.save!(validate: false)
exchange = Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
us = Exchanges::BinanceUs.create!(name: 'Binance.US', maker_fee: '0.1', taker_fee: '0.1')
key = ApiKey.new(user:, exchange:, key: 'synthetic', secret: 'synthetic', status: :correct, key_type: :read_only); key.save!(validate: false)
ms = 1_786_960_800_000
base = { 'id' => 'd1', 'txId' => 'hash', 'coin' => 'BTC', 'network' => 'BTC', 'address' => 'addr', 'addressTag' => '',
         'amount' => '1', 'insertTime' => ms, 'status' => 1 }
reward = { 'asset' => 'BTC', 'time' => ms, 'rewards' => '1', 'projectId' => 'p1', 'positionId' => 'pos1', 'type' => 'BONUS' }
earn = reward.merge('purchaseAmount' => '1', 'amount' => '1')
dust = { 'transId' => 'detail', 'fromAsset' => 'BTC', 'amount' => '1', 'transferedAmount' => '0.1', 'serviceChargeAmount' => '0.01' }
parent = ->(id, *details) { { 'transId' => id, 'operateTime' => ms, 'userAssetDribbletDetails' => details } }
withdrawal = base.merge('id' => 'w1', 'status' => 6, 'applyTime' => '2026-08-17 10:00:00', 'transactionFee' => '0.01')
cases = [
  ['deposits_same', { deposit_history: [base, base.merge('id' => 'd2')] }],
  ['deposits_coins_networks', { deposit_history: [base, base.merge('id' => 'd2', 'coin' => 'ETH', 'network' => 'ETH')] }],
  ['withdrawals', { withdraw_history: [withdrawal, withdrawal.merge('id' => 'w2')] }],
  ['deposit_withdrawal', { deposit_history: [base], withdraw_history: [withdrawal] }],
  ['locked_rewards', { simple_earn_locked_rewards: { 'rows' => [reward, reward.merge('positionId' => 'pos2')], 'total' => 2 } }],
  ['flexible_missing_type', { simple_earn_flexible_rewards: { 'rows' => [reward, reward.merge('type' => 'REALTIME'), reward.merge('projectId' => 'p2')], 'total' => 3 } }],
  ['dust_assets', { dust_log: { 'userAssetDribblets' => [parent.('parent1', dust, dust.merge('fromAsset' => 'ETH'))] } }],
  ['dust_parents', { dust_log: { 'userAssetDribblets' => [parent.('parent1', dust), parent.('parent2', dust.merge('fromAsset' => 'ETH'))] } }]
]
%i[simple_earn_flexible_subscriptions simple_earn_flexible_redemptions simple_earn_locked_subscriptions simple_earn_locked_redemptions].each do |endpoint|
  id = endpoint.to_s.end_with?('subscriptions') ? 'purchaseId' : 'redeemId'
  cases << [endpoint.to_s, { endpoint => { 'rows' => [earn.merge(id => 'one'), earn.merge(id => 'two')], 'total' => 2 } }]
  cases << ["#{endpoint}_fallback", { endpoint => { 'rows' => [earn, earn.merge('asset' => 'ETH', 'projectId' => 'p2')], 'total' => 2 } }]
end
interest = { 'asset' => 'BTC', 'interest' => '0.01', 'interestAccuredTime' => ms, 'txId' => 'i1', 'type' => 'PERIODIC', 'isolatedSymbol' => 'BTCUSDT' }
liquidation = { 'asset' => 'BTC', 'symbol' => 'BTCUSDT', 'orderId' => 1, 'qty' => '1', 'quoteAsset' => 'USDT', 'quoteQty' => '10', 'updatedTime' => ms }
income = { 'asset' => 'BTC', 'income' => '1', 'tranId' => 1, 'time' => ms, 'incomeType' => 'REALIZED_PNL' }
cases += [['margin_interest_mapper_only', { margin_interest_history: { 'rows' => [interest, interest.merge('txId' => 'i2', 'type' => 'ON_BORROW', 'isolatedSymbol' => 'BTCETH')] } }],
          ['liquidations', { margin_force_liquidation: { 'rows' => [liquidation, liquidation.merge('symbol' => 'BTCETH')] } }]]
%i[futures_income_history coin_futures_income_history].each { |e| cases << [e.to_s, { e => [income, income.merge('incomeType' => 'FUNDING_FEE')] }] }
cases << ['flexible_mapper_only', cases.find { |n, _| n == 'flexible_missing_type' }.last]
%w[deposits_same withdrawals].each { |name| cases << ["us_#{name}", cases.find { |n, _| n == name }.last] }
cases << ['negative_controls', {
  convert_trade_flow: { 'list' => [{ 'quoteId' => 'q1', 'fromAsset' => 'BTC', 'toAsset' => 'ETH', 'fromAmount' => '1', 'toAmount' => '2', 'createTime' => ms }] },
  fiat_payments: { 'data' => [{ 'orderNo' => 'f1', 'cryptoCurrency' => 'BTC', 'fiatCurrency' => 'USD', 'obtainAmount' => '1', 'sourceAmount' => '10', 'totalFee' => '0.1', 'status' => 'Completed', 'createTime' => ms }] },
  asset_dividend: { 'rows' => [{ 'id' => 1, 'asset' => 'BTC', 'amount' => '1', 'divTime' => ms }] }
}]
dump = ->(rows) { rows.map { |r| r.transform_values { |v| v.is_a?(BigDecimal) ? v.to_s('F') : v.is_a?(Time) ? v.utc.iso8601(6) : v } } }
vectors = cases.map do |name, answers|
  AccountTransaction.delete_all
  key.exchange = name.start_with?('us_') ? us : exchange
  client.mapper_only = name.end_with?('mapper_only')
  client.answers = { deposit_history: [], withdraw_history: [], exchange_information: { 'symbols' => [] } }.merge(answers)
  entries = key.exchange.get_ledger(api_key: key, start_time: Time.utc(2026, 8, 17)).data
  if name == 'negative_controls'
    exchange.instance_variable_set(:@exchange_symbols_map, { 'BTCUSDT' => { base: 'BTC', quote: 'USDT' }, 'ETHBTC' => { base: 'ETH', quote: 'BTC' } })
    %w[BTCUSDT ETHBTC].each { |symbol| entries.concat(exchange.send(:normalize_trade, { 'id' => 1, 'orderId' => 2, 'qty' => '1', 'quoteQty' => '2', 'commission' => '0.01', 'commissionAsset' => 'BTC', 'time' => ms, 'isBuyer' => true }, symbol)) }
  end
  first = AccountTransactionSync.new(key).store!(entries)
  before = AccountTransaction.order(:id).map { |r| r.attributes.except('id', 'created_at', 'updated_at') }
  second = AccountTransactionSync.new(key).store!(entries.reverse)
  raise 'repeat changed stored rows' unless before == AccountTransaction.order(:id).map { |r| r.attributes.except('id', 'created_at', 'updated_at') }
  { name:, answers:, entries: dump.(entries), stored: before, imported: first[:imported], repeated: second[:imported] }
end

# Identical enriched inputs for both engines: fixed USD prices, UTC, FIFO, non-taxable crypto swaps.
# Complete equal-net-BNB pairs are the target, never the lossy legacy dust result.
row = ->(kind, coin, amount, value, day, group = nil) { { entry_type: kind, base_currency: coin, base_amount: amount.to_d,
  fiat_value: value.to_d, fee_fiat_value: 0.to_d, transacted_at: Time.utc(2026, 8, day), exchange: 'binance', group_id: group,
  price_missing: false, linked: false } }
history = [row.(:deposit,'BTC','4','40',1), row.(:buy,'ETH','4','80',2), row.(:staking_reward,'BTC','1','10',3),
           row.(:withdrawal,'BTC','1','10',4), row.(:deposit,'BTC','1','10',5), row.(:airdrop,'ETH','1','20',6),
           row.(:swap_out,'BTC','1','10',7,'convert'), row.(:swap_in,'ETH','0.5','10',7,'convert'),
           row.(:swap_out,'BTC','1','10',8,'dust_btc'), row.(:swap_in,'BNB','0.09','9',8,'dust_btc'),
           row.(:swap_out,'ETH','1','20',8,'dust_eth'), row.(:swap_in,'BNB','0.09','9',8,'dust_eth'),
           row.(:sell,'BTC','1','15',9), row.(:sell,'ETH','1','25',9), row.(:sell,'BNB','0.09','30',10)]
history.select { |r| r[:entry_type] == :swap_in && r[:base_currency] == 'BNB' }.each do |r|
  r.merge!(base_amount: '0.1'.to_d, fiat_value: '10'.to_d, fee_currency: 'BNB', fee_amount: '0.01'.to_d, fee_fiat_value: '1'.to_d)
end
accounting = [history, history.reject { |r| r[:group_id] == 'dust_eth' }].map do |rows|
  engine = Tracker::Ledger::Engine.new
  disposals = engine.calculate(rows, crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)
  lots = engine.lots.sort.to_h.transform_values { |ls| ls.map { |l| [l[:amount].to_s('F'), l[:cost_per_unit].to_s('F')] } }
  holdings = rows.flat_map { |r| Tracker::Ledger.quantity_moves(r) }.group_by(&:first).transform_values { |moves| moves.sum { |_, n| n }.to_s('F') }
  { rows: dump.(rows), lots:, holdings:, disposals: dump.(disposals), period: '2026', realised: disposals.sum { |d| d[:gain_loss] }.to_s('F') }
end
sources = %w[script/rust/record_binance_identity.rb app/models/exchanges/binance.rb app/models/exchanges/binance_us.rb
  app/services/account_transaction_sync.rb app/models/account_transaction.rb app/models/tracker/ledger.rb
  app/models/tax/methods/fifo.rb app/models/tax/methods/acquisition_fee.rb app/models/tax/methods/return_of_capital.rb db/schema.rb]
File.write(ARGV.fetch(0), JSON.pretty_generate({ sources: sources.to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }, collisions: vectors, accounting: }) + "\n")
