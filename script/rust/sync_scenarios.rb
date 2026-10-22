# The sync parity grid (script/rust/sync.rb loads this). One scenario per branch of Exchanges::Alpaca#get_ledger and its
# normalisers, AccountTransactionSync, AccountTransaction::SyncJob, AccountBalance::Sync and AccountBalance::SyncJob.
# A scenario is { name:, setup: ->(ctx) { rows the install starts with }, steps: [one job run each] }.
module SyncScenarios
  P = SyncParity
  module_function

  def split_pair(symbol, remove, add, date, prefix: symbol.downcase, type: 'SPLIT')
    [P.nta("#{prefix}-remove", type, symbol:, qty: remove, net_amount: '0', date:),
     P.nta("#{prefix}-add", type, symbol:, qty: add, net_amount: '0', date:)]
  end

  # Two split legs as text: each with a key twice at the top (`note`), twice inside an object, twice inside an object
  # inside an array, once under an escaped spelling of itself, and an array 98 deep (100 levels with the page and the leg).
  def duplicate_key_legs
    extra = '"note":"first","extra":{"x":1,"y":[{"k":1,"k":2}],"x":2,"\u0078":3},"note":"last","deep":' + ('[' * 98) + (']' * 98)
    ["{\"id\":\"dup-remove\",\"activity_type\":\"SPLIT\",\"symbol\":\"KLAC\",\"qty\":\"-10\",\"net_amount\":\"0\",\"date\":\"2026-09-15\",\"status\":\"executed\",#{extra}}",
     "{\"id\":\"dup-add\",\"activity_type\":\"SPLIT\",\"symbol\":\"KLAC\",\"qty\":\"100\",\"net_amount\":\"0\",\"date\":\"2026-09-15\",\"status\":\"executed\",#{extra}}"]
  end

  def ledger_scenarios
    night = P::NIGHT
    next_night = P::NEXT_NIGHT
    [
      # ---- every activity type, by family ----
      { name: 'ledger-fills',
        setup: lambda do |ctx|
          P.holder(ctx, 'AAPL', at: Time.utc(2026, 9, 10, 14, 29), amount: '0.459712230', external_id: 'ord-aapl-1')
          P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 13, 14, 29), amount: '1', external_id: 'fill-direct')
        end,
        steps: [P.ledger(night, [
          P.fill('f-1', 'AAPL', 'buy', '0.359712230', '166.80', '2026-09-10T14:30:00.123456Z', order_id: 'ord-aapl-1', type: 'partial_fill'),
          P.fill('f-2', 'AAPL', 'buy', '0.1', '166.85', '2026-09-10T14:30:00.987654321Z', order_id: 'ord-aapl-1'),
          P.fill('f-3', 'AAPL', 'sell', '0.2', '171.123456', '2026-09-11T15:00:00Z', order_id: 'ord-nobody'),
          P.fill('f-4', 'BTC/USD', 'buy', '0.001130165', '64149.97', '2026-09-12T03:04:05.5-04:00'),
          P.fill('f-5', 'ETHUSD', 'sell', '0.5', '2500.1', '2026-09-12T10:00:00Z'),
          P.fill('f-6', 'XYZUSD', 'buy', '3', '1.5', '2026-09-12T11:00:00Z'),
          P.fill('fill-direct', 'KLAC', 'buy', '1', '700', '2026-09-13T14:30:00Z', order_id: nil),
          P.fill('f-8', 'QQQM', 'buy', 0.25, 201.37, '2026-09-14T14:30:00Z'),
          P.fill('f-9', 'QQQM', 'sell_short', '0.000000001', '201.379999999', '2026-09-14T14:31:00Z')
        ])] },
      { name: 'ledger-cash_transfers',
        steps: [P.ledger(night, [
          P.nta('csd-1', 'CSD', net_amount: '5000.00', date: '2026-09-01', group_id: 'grp-1'),
          P.nta('csw-1', 'CSW', net_amount: '-1200.50', date: '2026-09-02'),
          P.nta('jnlc-in', 'JNLC', net_amount: '25', date: '2026-09-03'),
          P.nta('jnlc-out', 'JNLC', net_amount: '-30.10', date: '2026-09-03'),
          P.nta('oct-1', 'OCT', net_amount: '1', date: '2026-09-04'),
          P.nta('acatc-1', 'ACATC', net_amount: '-2', date: '2026-09-05'),
          P.nta('csd-time', 'CSD', net_amount: '7.5', transaction_time: '2026-09-06T12:13:14.000001Z'),
          P.nta('csd-zero', 'CSD', net_amount: nil, date: '2026-09-07')
        ])] },
      { name: 'ledger-income',
        steps: [P.ledger(night, [
          P.nta('div-1', 'DIV', net_amount: '1.23', symbol: 'AAPL', qty: '4', per_share_amount: '0.3075', date: '2026-09-01', group_id: 'pay-1'),
          P.nta('divcgl-1', 'DIVCGL', net_amount: '0.5', symbol: 'QQQM', date: '2026-09-02'),
          P.nta('divcgs-1', 'DIVCGS', net_amount: '0.25', symbol: 'QQQM', date: '2026-09-02'),
          P.nta('cgd-1', 'CGD', net_amount: '0.125', symbol: 'QQQM', date: '2026-09-03'),
          P.nta('divtxex-1', 'DIVTXEX', net_amount: '2', symbol: 'QQQM', date: '2026-09-03'),
          P.nta('div-reversal', 'DIV', net_amount: '-0.41', symbol: 'AAPL', date: '2026-09-04'),
          P.nta('int-1', 'INT', net_amount: '0.07', date: '2026-09-05'),
          P.nta('ptr-1', 'PTR', net_amount: '0.01', date: '2026-09-06'),
          P.nta('roc-qty', 'DIVROC', net_amount: '7.25', symbol: 'QQQM', qty: '2', date: '2026-09-07'),
          P.nta('roc-no-qty', 'DIVROC', net_amount: '-3.5', symbol: 'QQQM', date: '2026-09-08')
        ])] },
      { name: 'ledger-withholding_and_fees',
        steps: [P.ledger(night, [
          P.nta('divnra-1', 'DIVNRA', net_amount: '-0.18', symbol: 'AAPL', date: '2026-09-01', group_id: 'pay-1'),
          P.nta('divft-1', 'DIVFT', net_amount: '-0.2', symbol: 'AAPL', date: '2026-09-01'),
          P.nta('divtw-1', 'DIVTW', net_amount: '-0.3', symbol: 'AAPL', date: '2026-09-01'),
          P.nta('intnra-1', 'INTNRA', net_amount: '-0.02', date: '2026-09-02'),
          P.nta('inttw-1', 'INTTW', net_amount: '-0.03', symbol: nil, date: '2026-09-02'),
          P.nta('fee-1', 'FEE', net_amount: '-0.01', date: '2026-09-03'),
          P.nta('divfee-1', 'DIVFEE', net_amount: '-0.05', symbol: 'AAPL', date: '2026-09-03'),
          P.nta('ptc-1', 'PTC', net_amount: '-0.000123', date: '2026-09-03'),
          P.nta('cfee-1', 'CFEE', net_amount: '0', symbol: 'ETHUSD', qty: '-0.000195', price: '1884.5', date: '2026-09-04'),
          P.nta('cfee-2', 'CFEE', net_amount: '0', symbol: 'UNKNOWNUSD', qty: '-0.5', price: '10.0', date: '2026-09-04')
        ])] },
      { name: 'ledger-unsupported_and_canceled',
        steps: [P.ledger(night, [
          P.nta('ma-1', 'MA', symbol: 'KLAC', qty: '-3', net_amount: '2100.75', date: '2026-09-01'),
          P.nta('reo-1', 'REO', date: '2026-09-02'),
          P.nta('zqxx-1', 'ZQXX', symbol: 'AAPL', qty: 2, net_amount: 1.0e-05, date: '2026-09-03',
                                  description: 'A <b>new</b> & odd "type" — zażółć', detail: { 'legs' => [1, 2.5, nil, true], 'big' => 1.0e20 }),
          P.nta('div-canceled', 'DIV', net_amount: '9.99', symbol: 'AAPL', date: '2026-09-04', status: 'canceled'),
          P.fill('fill-canceled', 'AAPL', 'buy', '1', '100', '2026-09-05T14:30:00Z', status: 'canceled'),
          P.nta('int-correct', 'INT', net_amount: '0.5', date: '2026-09-06', status: 'correct')
        ])] },

      # ---- splits (Exchanges::Alpaca#normalize_split, #merge_split_entries; AccountTransactionSync#log_split) ----
      { name: 'ledger-split_forward',
        setup: lambda do |ctx|
          P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30))
          P.holder(ctx, 'AAPL', at: Time.utc(2026, 9, 2, 14, 30))
        end,
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-09-15'))] },
      { name: 'ledger-split_reverse',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30), amount: '30') },
        steps: [P.ledger(night, split_pair('KLAC', '-30', '10', '2026-09-15'))] },
      { name: 'ledger-split_three_legs',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [
          P.nta('ssp-remove', 'SSP', symbol: 'KLAC', qty: '-10', net_amount: '0', date: '2026-09-15'),
          P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15'),
          P.nta('ssp-add-2', 'SPLIT', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')
        ])] },
      # A group that grows: the pair one night, the pair and a third leg the next. Rails reads the group as a duplicate of
      # the pair it stored (the same first id) and keeps +5 and 3:2; the third leg is never counted. (A Rails defect, ported.)
      { name: 'ledger-split_third_leg_later',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [
          P.nta('ssp-remove', 'SSP', symbol: 'KLAC', qty: '-10', net_amount: '0', date: '2026-09-15'),
          P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')
        ]), P.ledger(next_night, [
          P.nta('ssp-remove', 'SSP', symbol: 'KLAC', qty: '-10', net_amount: '0', date: '2026-09-15'),
          P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15'),
          P.nta('ssp-add-2', 'SPLIT', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')
        ])] },
      # A split quantity no ratio can be made of: Ruby raises FloatDomainError inside #get_ledger, the job records it and
      # re-raises, and nothing of the read is stored. Rust refuses the number before any arithmetic; only the recorded
      # text differs. (Listed divergence.)
      { name: 'ledger-split_hostile_quantity',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [P.nta('int-before', 'INT', net_amount: '0.07', date: '2026-09-10'),
                                 P.nta('klac-remove', 'SPLIT', symbol: 'KLAC', qty: '-1', net_amount: '0', date: '2026-09-15'),
                                 P.nta('klac-add', 'SPLIT', symbol: 'KLAC', qty: '1e400', net_amount: '0', date: '2026-09-15')])] },
      { name: 'ledger-split_fractional',
        setup: ->(ctx) { P.holder(ctx, 'QQQM', at: Time.utc(2026, 9, 2, 14, 30), amount: '3.333') },
        steps: [P.ledger(night, split_pair('QQQM', '-3.333', '4.9995', '2026-09-15') +
                                split_pair('AAPL', '-1000', '1001', '2026-09-16') + split_pair('KLAC', '-5', '5', '2026-09-17'))] },
      { name: 'ledger-split_unmerged',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [
          P.nta('lone-add', 'SPLIT', symbol: 'KLAC', qty: '90', net_amount: '0', date: '2026-09-10'),
          P.nta('int-between', 'INT', net_amount: '0.07', date: '2026-09-11'),
          P.nta('d1-remove', 'SPLIT', symbol: 'AAPL', qty: '-1', net_amount: '0', date: '2026-09-12'),
          P.nta('d2-add', 'SPLIT', symbol: 'AAPL', qty: '4', net_amount: '0', date: '2026-09-13')
        ])] },
      # Two legs of one split with other activities between them. Rails merges legs only while they are consecutive, so
      # it stores them as two rows with no ratio and bumps the bot twice (a Rails defect, ported: see the plan).
      { name: 'ledger-split_legs_apart',
        setup: ->(ctx) { P.holder(ctx, 'QQQM', at: Time.utc(2026, 9, 2, 14, 30), amount: '2') },
        steps: [P.ledger(night, [
          P.nta('x-remove', 'SPLIT', symbol: 'QQQM', qty: '-2', net_amount: '0', date: '2026-09-14'),
          P.nta('y-remove', 'SPLIT', symbol: 'KLAC', qty: '-100', net_amount: '0', date: '2026-09-14'),
          P.nta('int-between', 'INT', net_amount: '0.07', date: '2026-09-14'),
          P.nta('x-add', 'SPLIT', symbol: 'QQQM', qty: '4', net_amount: '0', date: '2026-09-14')
        ])] },
      # A stored pair, then a read that holds one stored leg and one new leg, in either order. Rails knows a stored group
      # by its first id only: with the new leg first it stores the group beside the pair (the shared leg counted twice);
      # with the stored leg first it skips the group (the new leg lost). (Rails defects, ported: see the plan.)
      { name: 'ledger-split_overlap_new_leg_first',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [P.nta('ssp-remove', 'SSP', symbol: 'KLAC', qty: '-10', net_amount: '0', date: '2026-09-15'),
                                 P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')]),
                P.ledger(next_night, [P.nta('ssp-add-2', 'SPLIT', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15'),
                                      P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')])] },
      # Two genuine splits of one symbol on one date, back to back: 10 become 20, then 20 become 60. Rails merges all four
      # legs into one row and makes one ratio of them, 8:3, where the position went 6:1. (A Rails defect, ported.)
      { name: 'ledger-split_two_on_one_date',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [P.nta('one-remove', 'SPLIT', symbol: 'KLAC', qty: '-10', net_amount: '0', date: '2026-09-15'),
                                 P.nta('one-add', 'SPLIT', symbol: 'KLAC', qty: '20', net_amount: '0', date: '2026-09-15'),
                                 P.nta('two-remove', 'SPLIT', symbol: 'KLAC', qty: '-20', net_amount: '0', date: '2026-09-15'),
                                 P.nta('two-add', 'SPLIT', symbol: 'KLAC', qty: '60', net_amount: '0', date: '2026-09-15')])] },
      # The same, with ratios near one: 1000 become 1002, then 1002 become 1005. One row of +5 whose ratio, 287:286, is
      # (1002 + 1005) : (1000 + 1002). The position went 1.005; the stored ratio is 1.5 per mille off it, which a
      # reader can still tell from the row's own numbers. (A Rails defect, ported.)
      { name: 'ledger-split_two_near_one_on_one_date',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30), amount: '1000') },
        steps: [P.ledger(night, [P.nta('one-remove', 'SPLIT', symbol: 'KLAC', qty: '-1000', net_amount: '0', date: '2026-09-15'),
                                 P.nta('one-add', 'SPLIT', symbol: 'KLAC', qty: '1002', net_amount: '0', date: '2026-09-15'),
                                 P.nta('two-remove', 'SPLIT', symbol: 'KLAC', qty: '-1002', net_amount: '0', date: '2026-09-15'),
                                 P.nta('two-add', 'SPLIT', symbol: 'KLAC', qty: '1005', net_amount: '0', date: '2026-09-15')])] },
      # And one no reader of the row can tell: 1000 become 100 (a reverse split), then 100 become 1005. One row of +5
      # whose ratio is (100 + 1005) : (1000 + 100), within a per mille of the 1.005 the position went: it looks like one
      # split of 182:181, and is two. (A Rails defect, ported.)
      { name: 'ledger-split_two_alike_on_one_date',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30), amount: '1000') },
        steps: [P.ledger(night, [P.nta('one-remove', 'SPLIT', symbol: 'KLAC', qty: '-1000', net_amount: '0', date: '2026-09-15'),
                                 P.nta('one-add', 'SPLIT', symbol: 'KLAC', qty: '100', net_amount: '0', date: '2026-09-15'),
                                 P.nta('two-remove', 'SPLIT', symbol: 'KLAC', qty: '-100', net_amount: '0', date: '2026-09-15'),
                                 P.nta('two-add', 'SPLIT', symbol: 'KLAC', qty: '1005', net_amount: '0', date: '2026-09-15')])] },
      # A leg that comes again under the same id with another quantity (a correction): the group's first id is stored,
      # so Rails skips the group and the correction is not applied. (A Rails defect, ported.)
      { name: 'ledger-split_leg_changed',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-09-15')),
                P.ledger(next_night, split_pair('KLAC', '-10', '200', '2026-09-15'))] },
      { name: 'ledger-split_overlap_new_leg_last',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [P.nta('ssp-remove', 'SSP', symbol: 'KLAC', qty: '-10', net_amount: '0', date: '2026-09-15'),
                                 P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')]),
                P.ledger(next_night, [P.nta('ssp-add-1', 'SSP', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15'),
                                      P.nta('ssp-add-2', 'SPLIT', symbol: 'KLAC', qty: '15', net_amount: '0', date: '2026-09-15')])] },
      { name: 'ledger-split_across_pages',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 1, 2, 14, 30)) },
        steps: [P.ledger(night, P.filler(99) + [split_pair('KLAC', '-10', '100', '2026-04-10')[0]],
                         [split_pair('KLAC', '-10', '100', '2026-04-10')[1]] + P.filler(3, prefix: 'late', from: Time.utc(2026, 5, 1)))] },
      { name: 'ledger-split_add_then_pair',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [split_pair('KLAC', '-10', '100', '2026-09-15')[1]]),
                P.ledger(next_night, split_pair('KLAC', '-10', '100', '2026-09-15'))] },
      { name: 'ledger-split_remove_then_pair',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, [split_pair('KLAC', '-10', '100', '2026-09-15')[0]]),
                P.ledger(next_night, split_pair('KLAC', '-10', '100', '2026-09-15'))] },
      { name: 'ledger-split_pair_then_leg',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-09-15')),
                P.ledger(next_night, [split_pair('KLAC', '-10', '100', '2026-09-15')[1]])] },
      # A split dated ahead of today, then the next night, served as Alpaca serves `after`. The watermark stops at the
      # first sync's start rather than following the split into the future, so the second request asks from that start
      # less 25 h and the fill and the interest in between arrive.
      { name: 'ledger-split_future',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-10-05')),
                P.ledger(next_night, [P.fill('f-between', 'AAPL', 'buy', '1', '170', '2026-09-20T14:30:00Z'), P.nta('int-between', 'INT', net_amount: '0.07', date: '2026-09-21')] +
                                     split_pair('KLAC', '-10', '100', '2026-10-05')).merge('server_filters_after' => true)] },
      # A split of a symbol no bot traded (the only bot buys BTC): the row is stored and no bot is touched.
      { name: 'ledger-split_untraded_symbol',
        setup: ->(ctx) { P.holder(ctx, 'BTC', at: Time.utc(2026, 9, 2, 14, 30), amount: '0.001') },
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-09-15'))] },
      { name: 'ledger-split_old',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 4, 2, 14, 30)) },
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-05-01'))] },
      { name: 'ledger-split_holders',
        setup: lambda do |ctx|
          before = Time.utc(2026, 9, 2, 14, 30)
          P.holder(ctx, 'KLAC', at: before) # 1: holds, told
          sold_out = P.holder(ctx, 'KLAC', at: before) # 2: bought and sold everything before the split: quiet, still bumped
          Transaction.insert!(Transaction.where(bot_id: sold_out.id).first.attributes.except('id').merge('side' => 1, 'external_id' => 'sold-out', 'created_at' => before + 1.day))
          part = P.holder(ctx, 'KLAC', at: before) # 3: sold part: told
          Transaction.insert!(Transaction.where(bot_id: part.id).first.attributes.except('id').merge('side' => 1, 'external_id' => 'sold-part', 'amount' => 4, 'amount_exec' => 4,
                                                                                                    'quote_amount_exec' => 400, 'created_at' => before + 1.day))
          P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 16, 14, 30)) # 4: bought after the split: quiet, bumped
          P.holder(ctx, 'AAPL', at: before) # 5: another symbol: untouched
          failed = P.holder(ctx, 'KLAC', at: before) # 6: only a failed order: untouched
          Transaction.where(bot_id: failed.id).update_all(status: 1)
          # 7: an order cancelled before it filled: quiet, bumped. (Not an open order: a stopped stock bot with one is
          # work the Rust engine refuses, so its guard would refuse every write on this install.)
          unfilled = P.holder(ctx, 'KLAC', at: before)
          Transaction.where(bot_id: unfilled.id).update_all(external_status: 3, amount_exec: nil, quote_amount_exec: nil)
          cancelled = P.holder(ctx, 'KLAC', at: before) # 8: cancelled after filling part: told
          Transaction.where(bot_id: cancelled.id).update_all(external_status: 3, amount_exec: 2.5)
          # 9: flat, but its trades span an earlier split: told.
          spans = P.holder(ctx, 'KLAC', at: Time.utc(2026, 6, 1, 14, 30), amount: '1')
          Transaction.insert!(Transaction.where(bot_id: spans.id).first.attributes.except('id').merge('side' => 1, 'external_id' => 'spans-sell', 'created_at' => Time.utc(2026, 8, 1)))
          P.stored(ctx, entry_type: 15, base_currency: 'KLAC', base_amount: 1, tx_id: 'earlier-split', transacted_at: Time.utc(2026, 7, 1),
                        raw_data: { 'activity_type' => 'SPLIT', 'corporate_action' => 'split', 'split_ratio' => '2:1' })
          # 10: the order is recorded under another spelling but with the asset the venue lists as KLAC: told.
          renamed = P.holder(ctx, 'KLAC', at: before)
          Transaction.where(bot_id: renamed.id).update_all(base: 'KLA')
          # 11: another user's bot: untouched.
          other = User.new(name: 'Other', email: 'other@example.com', password: 'correct horse battery staple', confirmed_at: Time.current, setup_completed: true)
          other.save!(validate: false)
          P.holder(ctx.merge(user: other), 'KLAC', at: before)
          # An earlier line for the same instant: this split's own (upgraded), and another symbol's (left alone).
          BotActivityLog.insert!({ bot_id: 3, event: 'asset_split', level: 0, details: { 'base' => 'KLAC' }, created_at: Time.utc(2026, 9, 15) })
          BotActivityLog.insert!({ bot_id: 1, event: 'asset_split', level: 0, details: { 'base' => 'AAPL', 'ratio' => '4:1' }, created_at: Time.utc(2026, 9, 15) })
        end,
        steps: [P.ledger(night, split_pair('KLAC', '-10', '100', '2026-09-15'))] },

      # ---- pagination (#get_ledger) ----
      { name: 'ledger-pages_three', steps: [P.ledger(night, P.filler(100), P.filler(100, prefix: 'p2', from: Time.utc(2026, 4, 11)), P.filler(5, prefix: 'p3', from: Time.utc(2026, 7, 20)))] },
      { name: 'ledger-pages_exact_then_empty', steps: [P.ledger(night, P.filler(100), [])] },
      { name: 'ledger-pages_stalled', steps: [P.ledger(night, P.filler(100), P.filler(100))] },
      { name: 'ledger-page_two_fails', steps: [P.ledger(night, P.filler(100), P::SERVER_ERROR)] },
      { name: 'ledger-body_is_an_object', steps: [P.ledger(night, P.ok({}))] },

      # ---- failures (Clients::Alpaca#with_rescue, AccountTransaction::SyncJob#perform) ----
      { name: 'ledger-unauthorized', steps: [P.ledger(night, P::UNAUTHORIZED)] },
      { name: 'ledger-html_error', steps: [P.ledger(night, { 'status' => 502, 'body' => '<!DOCTYPE html><html><body>Bad gateway 1234567890</body></html>' })] },
      # Integers no double holds, passed through into raw_data: the body is given as text so that neither half of the
      # harness rounds it on the way in. (A fraction longer than a double is a Float in Ruby too.)
      { name: 'ledger-raw_large_integer',
        steps: [P.ledger(night, { 'status' => 200, 'body' => '[{"id":"big-1","activity_type":"INT","net_amount":"0.07","date":"2026-09-10","status":"executed",' \
                                                             '"reference":18446744073709551617,"nested":{"ids":[-9223372036854775809,123456789012345678901234567890]},' \
                                                             '"long_fraction":1.0000000000000000001,"plain":7}]' })] },
      # Floats and strings in raw_data are stored as the app's JSON encoder (Oj) writes what its parser read, not as the
      # venue wrote them: sixteen significant digits, `1.0` for a whole number, C's exponent form, `0.0` for a negative
      # zero, `<` for `<`. The body is given as text so that both halves read the same bytes.
      { name: 'ledger-raw_floats',
        steps: [P.ledger(night, { 'status' => 200, 'body' => '[{"id":"float-1","activity_type":"INT","net_amount":"0.07","date":"2026-09-10","status":"executed",' \
                                                             '"a":0.30000000000000004,"b":1.2345678901234567,"c":1.0,"d":1e2,"e":-0.0,"f":1E-7,"g":1e20,"h":1e15,' \
                                                             '"i":123456789012345678.0,"j":4.9e-324,"k":1.7976931348623157e308,"l":1984.0207455399993,"m":-0,"n":1.50,' \
                                                             '"o":[0.1,2.5e-5,{"p":1234567890123456.7}],"s":"a<b>&c \u00e9 \/ \u2028 \t \u0001 \ud83d\ude00","t<":null},' \
                                                             '{"id":"float-2","activity_type":"FILL","transaction_time":"2026-09-10T14:30:00Z","type":"fill","price":170.10,' \
                                                             '"qty":0.30000000000000004,"side":"buy","symbol":"AAPL","order_id":"o-float-2"}]' })] },
      # A value that is not JSON in a position a later value of the same key overwrites: the app's parser refuses the
      # whole body (the fetch fails with the body as its text), and so does Rust's reader.
      { name: 'ledger-malformed_overwritten',
        steps: [P.ledger(night, { 'status' => 200, 'body' => '[{"id":"m-1","activity_type":"INT","net_amount":"0.07","date":"2026-09-10","status":"executed","extra":{"qty":wat,"qty":"10"}}]' })] },
      # A split whose legs carry a key twice, at the top and nested, and an array nested to Ruby's limit exactly (the
      # page and the activity are two levels, then 98 more). Ruby's parser keeps the last value in the first place, at
      # every level, and that is what Rails stores; Rust stores the same. The second night returns one leg again, which
      # is a duplicate only to a reader that can read `merged_activity_ids` out of the stored row.
      { name: 'ledger-split_nested_duplicate_keys',
        setup: ->(ctx) { P.holder(ctx, 'KLAC', at: Time.utc(2026, 9, 2, 14, 30)) },
        steps: [P.ledger(night, { 'status' => 200, 'body' => "[#{SyncScenarios.duplicate_key_legs.join(',')}]" }),
                P.ledger(next_night, { 'status' => 200, 'body' => "[#{SyncScenarios.duplicate_key_legs.last}]" })] },
      # One level more: Ruby's parser refuses the page ("Too deeply nested"), Faraday reports a parsing error and the
      # fetch fails with the body as its message, like any body that is not JSON. Rust reads it the same way.
      { name: 'ledger-nesting_over_limit',
        steps: [P.ledger(night, { 'status' => 200, 'body' => "[{#{'"id":"deep-1","activity_type":"INT","net_amount":"0.07","date":"2026-09-10","status":"executed","deep":'}#{'[' * 99}#{']' * 99}}]" })] },
      { name: 'ledger-unreadable_body', steps: [P.ledger(night, { 'status' => 200, 'body' => 'upstream connect error' })] },
      { name: 'ledger-long_error',
        steps: [P.ledger(night, { 'status' => 403, 'body' => { 'message' => "forbidden for owner@example.com at https://paper-api.alpaca.markets/v2/account/activities?page_token=abc&x=1 #{'e' * 70} token 9f8e7d6c5b4a39281706f5e4d3c2b1a0 account 123456789012 #{'z' * 80}" } })] },
      { name: 'ledger-network_pre_send', steps: [P.ledger(night, P::PRE_SEND)] },
      { name: 'ledger-network_post_send', steps: [P.ledger(night, P.filler(100), P::POST_SEND)] },
      { name: 'ledger-certificate', steps: [P.ledger(night, P::CERTIFICATE)] },
      { name: 'ledger-failure_keeps_rows_and_watermark',
        setup: lambda do |ctx|
          ctx[:key].update_columns(last_synced_at: Time.utc(2026, 9, 10, 14, 30), last_sync_error: nil)
          P.stored(ctx, entry_type: 11, base_currency: 'USD', base_amount: '0.07', tx_id: 'int-old', transacted_at: Time.utc(2026, 9, 10))
        end,
        steps: [P.ledger(night, P::SERVER_ERROR)] },

      # ---- the watermark (AccountTransactionSync#sync!) ----
      { name: 'ledger-first_sync_empty', steps: [P.ledger(night, [])] },
      { name: 'ledger-first_sync_empty_clears_error',
        setup: ->(ctx) { ctx[:key].update_columns(last_sync_error: 'unauthorized.') },
        steps: [P.ledger(night, [])] },
      { name: 'ledger-idle',
        setup: ->(ctx) { ctx[:key].update_columns(last_synced_at: Time.utc(2026, 9, 10, 14, 30, 0, 123_456)) },
        steps: [P.ledger(night, [])] },
      { name: 'ledger-idle_clears_error',
        setup: ->(ctx) { ctx[:key].update_columns(last_synced_at: Time.utc(2026, 9, 10, 14, 30), last_sync_error: 'internal server error') },
        steps: [P.ledger(night, [])] },
      { name: 'ledger-rerun',
        steps: [P.ledger(night, [P.fill('f-1', 'AAPL', 'buy', '1', '166.80', '2026-09-10T14:30:00.123456789Z'), P.nta('int-1', 'INT', net_amount: '0.07', date: '2026-09-12')]),
                P.ledger(next_night, [P.fill('f-1', 'AAPL', 'buy', '1', '166.80', '2026-09-10T14:30:00.123456789Z'), P.nta('int-1', 'INT', net_amount: '0.07', date: '2026-09-12')])] },
      { name: 'ledger-new_rows_advance',
        setup: lambda do |ctx|
          ctx[:key].update_columns(last_synced_at: Time.utc(2026, 9, 10, 14, 30))
          P.stored(ctx, entry_type: 0, base_currency: 'AAPL', base_amount: 1, quote_currency: 'USD', quote_amount: '166.8', tx_id: 'f-1', transacted_at: Time.utc(2026, 9, 10, 14, 30))
        end,
        steps: [P.ledger(night, [P.fill('f-1', 'AAPL', 'buy', '1', '166.80', '2026-09-10T14:30:00Z'), P.fill('f-2', 'AAPL', 'buy', '2', '170', '2026-09-19T14:30:00.5Z')])] },
      { name: 'ledger-watermark_boundary',
        setup: lambda do |ctx|
          at = Time.utc(2026, 9, 10, 14, 30)
          ctx[:key].update_columns(last_synced_at: at)
          # Stored by an earlier sync, exactly at the watermark.
          P.stored(ctx, entry_type: 11, base_currency: 'USD', base_amount: '0.07', tx_id: 'at-watermark', transacted_at: at)
          # Stored by a file import, with no id: the venue's copy now arrives with one, half a second later.
          P.stored(ctx, key: nil, entry_type: 4, base_currency: 'USD', base_amount: '250', tx_id: nil, transacted_at: at + 10.minutes)
          # The same, a full second apart: a row of its own.
          P.stored(ctx, key: nil, entry_type: 4, base_currency: 'USD', base_amount: '300', tx_id: nil, transacted_at: at + 20.minutes)
          # The same instant and amount under another type: not the same event.
          P.stored(ctx, key: nil, entry_type: 5, base_currency: 'USD', base_amount: '400', tx_id: nil, transacted_at: at + 30.minutes)
        end,
        steps: [P.ledger(night, [
          P.nta('at-start', 'INT', net_amount: '0.01', transaction_time: '2026-09-09T13:30:00Z'), # exactly at `after` (watermark - 25 h)
          P.nta('at-watermark', 'INT', net_amount: '0.07', transaction_time: '2026-09-10T14:30:00Z'),
          P.nta('dep-250', 'CSD', net_amount: '250', transaction_time: '2026-09-10T14:40:00.5Z'),
          P.nta('dep-300', 'CSD', net_amount: '300', transaction_time: '2026-09-10T14:50:01Z'),
          P.nta('dep-400', 'CSD', net_amount: '400', transaction_time: '2026-09-10T15:00:00Z'),
          P.nta('just-after', 'INT', net_amount: '0.02', transaction_time: '2026-09-10T14:30:00.000001Z')
        ])] },
      { name: 'ledger-skipped_rows_hold_watermark',
        setup: ->(ctx) { ctx[:key].update_columns(last_sync_error: 'StandardError: API error') },
        steps: [P.ledger(night, [
          P.nta('good-early', 'INT', net_amount: '1', date: '2026-05-15'),
          P.nta('split-no-symbol', 'SPLIT', qty: '5', net_amount: '0', date: '2026-05-16'),
          P.nta('roc-no-symbol', 'DIVROC', qty: '2', net_amount: '7.25', date: '2026-05-17'),
          P.nta('int-no-time', 'INT', net_amount: '1.15'),
          P.fill('fill-no-symbol', nil, 'buy', '1', '2', '2026-05-17T10:00:00Z'),
          P.nta('cfee-blank-symbol', 'CFEE', symbol: ' ', qty: '-1', date: '2026-05-17'),
          P.nta('good-late', 'INT', net_amount: '2.35', date: '2026-05-18')
        ])] },
      { name: 'ledger-ids_blank_or_missing',
        steps: [P.ledger(night, [
          P.nta('', 'INT', net_amount: '0.07', date: '2026-09-01'),
          P.nta(nil, 'INT', net_amount: '0.07', date: '2026-09-01'),
          P.nta(nil, 'INT', net_amount: '0.08', date: '2026-09-01'),
          P.nta('  ', 'INT', net_amount: '0.09', date: '2026-09-02')
        ])] },

      # ---- TransferMatcher.run! (the job's second step) ----
      { name: 'ledger-transfer_link',
        setup: lambda do |ctx|
          kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
          P.stored(ctx, exchange: kraken, key: nil, entry_type: 4, base_currency: 'USD', base_amount: '990', tx_id: 'k-dep-late', transacted_at: Time.utc(2026, 9, 12, 9))
          P.stored(ctx, exchange: kraken, key: nil, entry_type: 4, base_currency: 'USD', base_amount: '985.5', tx_id: 'k-dep', transacted_at: Time.utc(2026, 9, 11, 9))
          P.stored(ctx, exchange: kraken, key: nil, entry_type: 4, base_currency: 'USD', base_amount: '979.99', tx_id: 'k-dep-small', transacted_at: Time.utc(2026, 9, 10, 9))
          P.stored(ctx, exchange: kraken, key: nil, entry_type: 4, base_currency: 'USD', base_amount: '500', tx_id: 'k-dep-500', transacted_at: Time.utc(2026, 9, 11, 9))
          P.stored(ctx, exchange: kraken, key: nil, entry_type: 5, base_currency: 'USD', base_amount: '500', tx_id: 'k-wd-rejected', transacted_at: Time.utc(2026, 9, 10, 9),
                        transfer_link_rejected: true)
        end,
        steps: [P.ledger(night, [
          P.nta('csw-1000', 'CSW', net_amount: '-1000', date: '2026-09-10'),
          P.nta('csw-1000b', 'CSW', net_amount: '-1000', date: '2026-09-10'),
          P.nta('csw-late', 'CSW', net_amount: '-500', date: '2026-09-14')
        ])] },

      # ---- asset identity (Exchanges::Alpaca#ledger_asset_ids, #resolve_recent_assets) ----
      { name: 'ledger-asset_ids',
        setup: lambda do |ctx|
          a = ctx[:alpaca]
          usd = ctx[:assets]['USD']
          mk = lambda do |external_id, symbol, category|
            Asset.create!(external_id:, symbol:, name: symbol, category:).tap { |x| ExchangeAsset.create!(exchange: a, asset: x, available: true) }
          end
          stock = { base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1' }
          # A replaced listing: the old KLAC asset under a tombstoned name. KLAC now names two assets.
          old_klac = mk.call('alpaca_old-klac', 'KLAC', 'Stock')
          Ticker.create!(exchange: a, ticker: '__stale_9_KLAC', base: '__stale_9_KLAC', quote: 'USD', base_asset: old_klac, quote_asset: usd, available: false, **stock)
          # A security spelled like a coin the venue lists.
          btc_trust = mk.call('BTC.US', 'BTC', 'Stock')
          Ticker.create!(exchange: a, ticker: '__stale_3_BTC', base: '__stale_3_BTC', quote: 'USDX', base_asset: btc_trust, quote_asset: usd, available: false, **stock)
          Asset.create!(external_id: 'solana', symbol: 'SOL', name: 'Solana', category: 'Cryptocurrency') # known by the curated map only
          P.stored(ctx, entry_type: 0, base_currency: 'QQQM', base_amount: 1, tx_id: 'recent-unresolved', transacted_at: Time.utc(2026, 9, 1),
                        raw_data: { 'activity_type' => 'FILL', 'symbol' => 'QQQM' }, created_at: Time.utc(2026, 9, 18), updated_at: Time.utc(2026, 9, 18))
          P.stored(ctx, entry_type: 0, base_currency: 'QQQM', base_amount: 1, tx_id: 'old-unresolved', transacted_at: Time.utc(2026, 8, 1),
                        raw_data: { 'activity_type' => 'FILL', 'symbol' => 'QQQM' }, created_at: Time.utc(2026, 9, 12), updated_at: Time.utc(2026, 9, 12))
          { 'file-usd' => 'USD', 'file-eth' => 'ETH', 'file-sol' => 'SOL', 'file-qqqm' => 'QQQM', 'file-klac' => 'KLAC', 'file-nothing' => 'NOPE' }.each do |tx_id, name|
            P.stored(ctx, key: nil, entry_type: 4, base_currency: name, base_amount: 1, tx_id:, transacted_at: Time.utc(2026, 9, 1),
                          created_at: Time.utc(2026, 9, 18), updated_at: Time.utc(2026, 9, 18))
          end
          P.stored(ctx, entry_type: 4, base_currency: 'AAPL', base_amount: 1, tx_id: 'kept-asset', transacted_at: Time.utc(2026, 9, 1), base_asset_id: ctx[:assets]['QQQM'].id,
                        raw_data: { 'activity_type' => 'JNLS', 'symbol' => 'AAPL' }, created_at: Time.utc(2026, 9, 18), updated_at: Time.utc(2026, 9, 18))
        end,
        steps: [P.ledger(night, [
          P.fill('a-stock', 'AAPL', 'buy', '1', '100', '2026-09-10T14:30:00Z'),
          P.fill('a-two-candidates', 'KLAC', 'buy', '1', '100', '2026-09-10T14:31:00Z'),
          P.fill('a-coin-slash', 'BTC/USD', 'buy', '0.01', '64000', '2026-09-10T14:32:00Z'),
          P.fill('a-coin-compact', 'ETHUSD', 'buy', '0.01', '2500', '2026-09-10T14:33:00Z'),
          P.fill('a-coin-curated', 'SOL/USD', 'buy', '1', '150', '2026-09-10T14:34:00Z'),
          P.fill('a-coin-curated-compact', 'SOLUSDT', 'buy', '1', '150', '2026-09-10T14:34:30Z'),
          P.fill('a-coin-unknown', 'NOPE/USD', 'buy', '1', '150', '2026-09-10T14:35:00Z'),
          P.fill('a-security-named-like-a-coin', 'BTC', 'buy', '2', '40', '2026-09-10T14:36:00Z'),
          P.fill('a-lowercase', 'aapl', 'buy', '2', '40', '2026-09-10T14:37:00Z'),
          P.nta('a-div', 'DIV', net_amount: '1', symbol: 'AAPL', date: '2026-09-11'),
          P.nta('a-cfee', 'CFEE', net_amount: '0', symbol: 'ETHUSD', qty: '-0.001', date: '2026-09-11'),
          P.nta('a-cfee-unknown', 'CFEE', net_amount: '0', symbol: 'SOL', qty: '-0.001', date: '2026-09-11'),
          P.nta('a-roc', 'DIVROC', net_amount: '1', symbol: 'QQQM', qty: '1', date: '2026-09-11'),
          P.nta('a-split', 'SPLIT', net_amount: '0', symbol: 'QQQM', qty: '1', date: '2026-09-11'),
          P.nta('a-unsupported-no-symbol', 'REO', date: '2026-09-11')
        ])] },

      # ---- which key reads (ApiKey.reading) ----
      { name: 'ledger-reading_prefers_trading',
        setup: lambda do |ctx|
          ApiKey.new(user: ctx[:user], exchange: ctx[:alpaca], key: 'PKREADONLY', secret: 'ro-secret', passphrase: 'paper', status: :correct, key_type: :read_only).save!(validate: false)
          ApiKey.new(user: ctx[:user], exchange: ctx[:alpaca], key: 'PKWITHDRAW', secret: 'wd-secret', passphrase: 'paper', status: :correct, key_type: :withdrawal).save!(validate: false)
        end,
        steps: [P.ledger(night, [])] },
      { name: 'ledger-reading_falls_back_to_read_only',
        setup: lambda do |ctx|
          ctx[:key].update_columns(status: ApiKey.statuses[:incorrect])
          ApiKey.new(user: ctx[:user], exchange: ctx[:alpaca], key: 'PKREADONLY', secret: 'ro-secret', passphrase: 'paper', status: :correct, key_type: :read_only).save!(validate: false)
        end,
        steps: [P.ledger(night, [])] }
    ]
  end

  def balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    next_night = '2026-09-21T02:30:03.250000Z'
    snaps = P.snapshot_body('AAPL' => 227.52, 'KLAC' => 701.5, 'QQQM' => 201.379999)
    coins = P.prices_body('bitcoin' => 64_321.123456789, 'ethereum' => 0.1 + 0.2)
    held = P.ok([P.position('AAPL', '10.5'), P.position('KLAC', '0.359712230'), P.position('QQQM', '3'),
                 P.position('BTCUSD', '0.001130165', asset_class: 'crypto'), P.position('ETHUSD', '1.25', asset_class: 'crypto')])
    seeded = lambda do |ctx|
      P.balance(ctx, 'USD', free: '90000', usd_price: 1, usd_value: 90_000)
      P.balance(ctx, 'AAPL', free: '10', usd_price: '220.5', usd_value: '2205')
      P.balance(ctx, 'BTC', free: '0.001', usd_price: '60000.12345678', usd_value: '60.00012346')
      P.balance(ctx, 'ETH', free: '2')
    end
    [
      *[true, false].product([true, false]).map do |coin_first, available|
        { name: "balances-collision-#{coin_first}-#{available}",
          setup: lambda do |ctx|
            stock = Asset.create!(external_id: 'BTC.US', symbol: 'BTC', name: 'Bitcoin Trust', category: 'Stock')
            ctx[:assets]['BTC_STOCK'] = stock
            ExchangeAsset.create!(exchange: ctx[:alpaca], asset: stock)
            ticker = Ticker.create!(exchange: ctx[:alpaca], base_asset: stock, quote_asset: ctx[:assets]['USD'],
                                   ticker: 'BTC', base: 'BTC', quote: 'USD', base_decimals: 9, quote_decimals: 2,
                                   price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
            ticker.update_column(:id, 0) unless coin_first
            ctx[:alpaca].tickers.update_all(available: available)
            P.balance(ctx, 'BTC_STOCK', free: '10')
            P.balance(ctx, 'BTC', free: '2')
          end,
          steps: [P.balances(night, account: P.account('100'),
                             positions: P.ok([P.position('BTC', '10'), P.position('BTCUSD', '2', asset_class: 'crypto')]),
                             snapshots: P.snapshot_body('BTC' => 30), prices: P.prices_body('bitcoin' => 60_000))] }
      end,
      { name: 'balances-excluded_positions', setup: seeded,
        steps: [P.balances(night, account: P.account('1'), positions: P.ok([
          P.position('FUTURE', 'unreadable', asset_class: 'future'), P.position('UNKNOWN', 'unreadable'),
          P.position('AAPL', '10'), P.position('BTCUSD', '2', asset_class: 'crypto')
        ]), snapshots: P.snapshot_body('AAPL' => 30), prices: P.prices_body('bitcoin' => 60_000))] },
      { name: 'balances-missing_class', setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('BTC', '10').except('asset_class')]))] },
      { name: 'balances-first', steps: [P.balances(night, account: P.account('99123.45'), positions: held, snapshots: snaps, prices: coins)] },
      { name: 'balances-updates_and_removes',
        setup: lambda do |ctx|
          seeded.call(ctx)
          P.balance(ctx, 'QQQM', free: '7', usd_price: '200', usd_value: '1400') # sold since: removed
          kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
          P.balance(ctx, 'BTC', free: '1', usd_price: '60000', usd_value: '60000', exchange: kraken) # another venue: untouched
          other = User.new(name: 'Other', email: 'other@example.com', password: 'correct horse battery staple', confirmed_at: Time.current, setup_completed: true)
          other.save!(validate: false)
          P.balance(ctx, 'AAPL', free: '1', usd_price: '220', usd_value: '220', user: other) # another user: untouched
        end,
        steps: [P.balances(night, account: P.account('99123.45'), positions: P.ok([P.position('AAPL', '10.5'), P.position('BTCUSD', '0.001130165', asset_class: 'crypto')]),
                                  snapshots: P.snapshot_body('AAPL' => 227.52), prices: P.prices_body('bitcoin' => 64_321.123456789))] },
      { name: 'balances-rerun',
        steps: [P.balances(night, account: P.account('99123.45'), positions: held, snapshots: snaps, prices: coins),
                P.balances(next_night, account: P.account('99123.45'), positions: held, snapshots: snaps, prices: coins)] },
      { name: 'balances-empty_account',
        setup: seeded,
        steps: [P.balances(night, account: P.account('0'))] },
      { name: 'balances-cash_only_null_cash',
        setup: seeded,
        steps: [P.balances(night, account: P.account(nil))] },
      { name: 'balances-skipped_positions',
        setup: lambda do |ctx|
          seeded.call(ctx)
          Asset.create!(external_id: 'NVDA.US', symbol: 'NVDA', name: 'Nvidia', category: 'Stock') # known, but the venue does not list it
        end,
        steps: [P.balances(night, account: P.account('100'),
                                  positions: P.ok([P.position('AAPL', '0'), P.position('KLAC', '-2'), P.position('NVDA', '5'), P.position('NOPE', '1'),
                                                   P.position('XYZUSD', '4', asset_class: 'crypto'), P.position('QQQM', '3'), P.position('QQQM', '4')]),
                                  snapshots: P.snapshot_body('QQQM' => 201))] },
      # Shares behind resting sells: free is what is left to sell, the rest locked, the holding whole (all ten
      # QQQM reserved still holds them). A position without qty_available fails the sync, as one without qty does.
      { name: 'balances-reserved_shares', setup: seeded,
        steps: [P.balances(night, account: P.account('100'),
                                  positions: P.ok([P.position('AAPL', '10.5', available: '4'), P.position('QQQM', '3', available: '0'),
                                                   P.position('BTCUSD', '2', asset_class: 'crypto', available: '0.5')]),
                                  snapshots: P.snapshot_body('AAPL' => 227.52, 'QQQM' => 201), prices: P.prices_body('bitcoin' => 60_000))] },
      { name: 'balances-missing_qty_available', setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('AAPL', '10').except('qty_available')]))] },
      { name: 'balances-null_qty_available', setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('AAPL', '10', available: nil)]))] },
      { name: 'balances-cash_symbol_position',
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('USD', '5')]))] },
      { name: 'balances-decimal_edges',
        steps: [P.balances(night, account: P.account('0.1234567890123456789'),
                                  positions: P.ok([P.position('AAPL', '123456.123456789'), P.position('KLAC', '0.000000001'), P.position('QQQM', '7'),
                                                   P.position('BTCUSD', '0.123456789', asset_class: 'crypto'), P.position('ETHUSD', '3', asset_class: 'crypto')]),
                                  snapshots: P.snapshot_body('AAPL' => 123_456_789.12345678, 'KLAC' => 2.675, 'QQQM' => '201.123456785'),
                                  prices: P.prices_body('bitcoin' => 1.0000000049999999, 'ethereum' => '2500.123456785'))] }, # Float#round(8) rounds this one up
      { name: 'balances-snapshot_gaps', # a held stock the snapshots do not answer for is asked of the market source
        setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('AAPL', '10.5'), P.position('QQQM', '3')]),
                                  snapshots: P.snapshot_body('AAPL' => 227.52), prices: P.prices_body('QQQM.US' => 199.5))] },
      # A snapshot with no latest trade. Rails reads its price as 0 and values the holding at nothing, freshly priced.
      # Rust reads no venue price: the market's, else the last one, else none. (Three listed divergences.)
      { name: 'balances-no_trade_market_price',
        setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('AAPL', '10.5')]),
                                  snapshots: P.ok('AAPL' => { 'dailyBar' => { 'c' => 1 } }), prices: P.prices_body('AAPL.US' => 226.4))] },
      { name: 'balances-no_trade_keeps_last_price',
        setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('AAPL', '10.5')]),
                                  snapshots: P.ok('AAPL' => { 'latestTrade' => { 'p' => 0 } }), prices: P.prices_body('bitcoin' => 1))] },
      { name: 'balances-no_trade_unpriced',
        steps: [P.balances(night, account: P.account('100'), positions: P.ok([P.position('KLAC', '2')]),
                                  snapshots: P.ok('KLAC' => { 'latestTrade' => nil }), prices: P.ok('data' => {}))] },
      { name: 'balances-snapshots_fail',
        setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: held, snapshots: P::SERVER_ERROR, prices: coins)] },
      { name: 'balances-market_fails',
        setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: held, snapshots: snaps, prices: P::SERVER_ERROR)] },
      { name: 'balances-market_partial',
        setup: seeded,
        steps: [P.balances(night, account: P.account('100'), positions: held, snapshots: snaps,
                                  prices: P.ok('data' => { 'bitcoin' => { 'usd' => nil }, 'ethereum' => { 'eur' => 2300 } }))] },
      { name: 'balances-account_unauthorized', setup: seeded, steps: [P.balances(night, account: P::UNAUTHORIZED)] },
      { name: 'balances-positions_fail', setup: seeded, steps: [P.balances(night, account: P.account('100'), positions: P::SERVER_ERROR)] },
      { name: 'balances-account_network', setup: seeded, steps: [P.balances(night, account: P::POST_SEND)] },
      { name: 'balances-account_certificate', setup: seeded, steps: [P.balances(night, account: P::CERTIFICATE)] },
      { name: 'balances-snapshots_network', setup: seeded, steps: [P.balances(night, account: P.account('100'), positions: held, snapshots: P::PRE_SEND)] },
      { name: 'balances-market_network', setup: seeded, steps: [P.balances(night, account: P.account('100'), positions: held, snapshots: snaps, prices: P::POST_SEND)] },
      # ---- answers that are not what #get_balances expects, against stored balances ----
      # Ruby raises on both of these and the job records the error: nothing is deleted. Rust refuses them too; only the
      # recorded text differs. (Listed divergences.)
      { name: 'balances-account_null', setup: seeded, steps: [P.balances(night, account: P.ok(nil), positions: held)] },
      { name: 'balances-positions_not_array', setup: seeded, steps: [P.balances(night, account: P.account('100'), positions: P.ok('positions' => 'later'))] },
      # No cash figure, a held position with no symbol, and one with no quantity: each fails the sync and removes
      # nothing (skipped or read as 0, the holding's stored balance would be deleted).
      { name: 'balances-account_no_cash', setup: seeded,
        steps: [P.balances(night, account: P.ok('id' => 'paper-account', 'status' => 'ACTIVE', 'buying_power' => '200000'), positions: held, snapshots: snaps, prices: coins)] },
      { name: 'balances-position_no_symbol', setup: seeded,
        steps: [P.balances(night, account: P.account('100'), prices: coins,
                                  positions: P.ok([P.position('AAPL', '10.5').except('symbol'), P.position('BTCUSD', '0.001130165', asset_class: 'crypto'),
                                                   P.position('ETHUSD', '1.25', asset_class: 'crypto')]))] },
      { name: 'balances-position_no_quantity', setup: seeded,
        steps: [P.balances(night, account: P.account('100'), prices: coins,
                                  positions: P.ok([P.position('AAPL', '10.5').except('qty'), P.position('BTCUSD', '0.001130165', asset_class: 'crypto'),
                                                   P.position('ETHUSD', '1.25', asset_class: 'crypto')]))] },
      { name: 'balances-clears_nothing_on_success',
        setup: lambda do |ctx|
          seeded.call(ctx)
          ctx[:key].update_columns(last_sync_error: 'internal server error', last_synced_at: Time.utc(2026, 9, 10, 14, 30))
        end,
        steps: [P.balances(night, account: P.account('100'))] }
    ]
  end

  # Exchanges::Binance#get_balances (and Binance.US's, the same code on api.binance.us) with AccountBalance::Sync.
  def binance_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    rates = P.ok('data' => { 'btc' => { 'value' => 1 }, 'usd' => { 'value' => 60_000.5 }, 'eur' => { 'value' => 55_000.25 } })
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25, 'tether' => 1.0001)
    held = P.binance_account(%w[BTC 0.50000000 0.10000000], %w[ETH 2.00000000], %w[USDT 100.50000000], %w[EUR 50.00], %w[LDBTC 1.00000000],
                             %w[BNB 3.00000000], %w[DOGE 0.00000000])
    install = ->(ctx) { P.binance(ctx) }
    cash = P.binance_account(%w[EUR 50], %w[BTC 0.1])
    eur_at = ->(usd) { P.ok('data' => { 'btc' => { 'value' => 1 }, 'usd' => { 'value' => usd }, 'eur' => { 'value' => 1 } }) }
    seeded = lambda do |ctx|
      P.binance(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2')
      P.balance(ctx, 'ETH', free: '1', usd_price: '2400', usd_value: '2400')
    end
    [
      # Cash by the market API's exchange rates, coins by its prices; an asset the venue lists no ticker for (LDBTC, BNB)
      # is not a balance; locked counts.
      { name: 'binance-balances-first', setup: install, steps: [P.binance_balances(night, account: held, rates:, prices: coins)] },
      { name: 'binance-balances-us', setup: ->(ctx) { P.binance(ctx, us: true) }, steps: [P.binance_balances(night, account: held, rates:, prices: coins)] },
      { name: 'binance-balances-locked_only', setup: install,
        steps: [P.binance_balances(night, account: P.binance_account(%w[BTC 0.00000000 0.25000000]), prices: coins)] },
      # The market fails: stored prices stay (stale), a holding that is gone is removed.
      { name: 'binance-balances-stale_and_gone', setup: seeded,
        steps: [P.binance_balances(night, account: P.binance_account(%w[BTC 0.45]), prices: P::SERVER_ERROR)] },
      # ---- failures: nothing written but the key's error (and its status, for a key the venue condemns) ----
      { name: 'binance-balances-invalid_key', setup: seeded,
        steps: [P.binance_balances(night, account: { 'status' => 401, 'body' => { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' } })] },
      { name: 'binance-balances-recv_window', setup: seeded,
        steps: [P.binance_balances(night, account: { 'status' => 400, 'body' => { 'code' => -1021, 'msg' => 'Timestamp for this request is outside of the recvWindow.' } })] },
      { name: 'binance-balances-throttled', setup: seeded,
        steps: [P.binance_balances(night, account: { 'status' => 429, 'body' => { 'code' => -1003, 'msg' => 'Too many requests; current limit of IP is 6000 requests per minute.' } })] },
      { name: 'binance-balances-bad_gateway', setup: seeded, steps: [P.binance_balances(night, account: { 'status' => 502, 'body' => '<html>502 Bad Gateway</html>' })] },
      { name: 'binance-balances-not_json', setup: seeded, steps: [P.binance_balances(night, account: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'binance-balances-network', setup: seeded,
        steps: [P.binance_balances(night, account: { 'network' => 'binance_pre_send', 'message' => 'Connection refused - connect(2) for "api.binance.com" port 443' })] },
      { name: 'binance-balances-no_balances', setup: seeded, steps: [P.binance_balances(night, account: P.ok('accountType' => 'SPOT'))] },
      { name: 'binance-balances-no_free', setup: seeded,
        steps: [P.binance_balances(night, account: P.ok('balances' => [{ 'asset' => 'BTC', 'locked' => '1' }]))] },
      # Listed divergences. A quantity that is no number: Ruby reads 0 and deletes the stored balance; Rust fails the sync.
      { name: 'binance-balances-garbage_free', setup: seeded,
        steps: [P.binance_balances(night, account: P.binance_account(%w[BTC abc], %w[ETH 1.5]), prices: coins)] },
      # The same divergence for a blank or whitespace quantity: Ruby's String#to_d reads 0 there too.
      { name: 'binance-balances-blank_free', setup: seeded,
        steps: [P.binance_balances(night, account: P.binance_account(['BTC', '', '0'], %w[ETH 1.5]), prices: coins)] },
      { name: 'binance-balances-whitespace_free', setup: seeded,
        steps: [P.binance_balances(night, account: P.binance_account(['BTC', '   ', '0'], %w[ETH 1.5]), prices: coins)] },
      # Listed divergences: a data-API number sent as a string is read only as a plain decimal. Ruby's String#to_f reads
      # "1_000" as 1000, "1_e2" as 1 and 256 zeros then 1 as 1; Rust refuses each and fails the sync.
      { name: 'binance-balances-rate_spelling', setup: install,
        steps: [P.binance_balances(night, account: cash, rates: P.ok('data' => { 'eur' => { 'value' => '1_000' }, 'usd' => { 'value' => 1100 } }),
                                   prices: coins)] },
      { name: 'binance-balances-price_underscore_exponent', setup: install,
        steps: [P.binance_balances(night, account: cash, rates: eur_at.call(1.1), prices: P.prices_body('bitcoin' => '1_e2'))] },
      { name: 'binance-balances-rate_zero_padded', setup: install,
        steps: [P.binance_balances(night, account: cash, rates: P.ok('data' => { 'eur' => { 'value' => "#{'0' * 256}1" }, 'usd' => { 'value' => 1.1 } }),
                                   prices: coins)] },
      # A plain decimal string is read as the number it spells, on both sides.
      { name: 'binance-balances-rate_plain_string', setup: install,
        steps: [P.binance_balances(night, account: cash, rates: P.ok('data' => { 'eur' => { 'value' => '1' }, 'usd' => { 'value' => '1.10' } }),
                                   prices: P.prices_body('bitcoin' => '60000.50'))] },
      # Repeat syncs: a fiat rate is held 12 hours (Utilities::Currency::STABLE_CACHE_DURATION). A minute later the
      # first rate is used and the rates are not asked for; 13 hours later they are asked for again.
      { name: 'binance-balances-rate_cached', setup: install,
        steps: [P.binance_balances(night, account: cash, rates: eur_at.call(1.1), prices: coins),
                P.binance_balances('2026-09-20T02:31:00.750000Z', account: cash, rates: eur_at.call(1.2), prices: coins),
                P.binance_balances('2026-09-20T15:31:00.750000Z', account: cash, rates: eur_at.call(1.3), prices: coins)] },
      # No exchange rate for cash: Ruby asks the coin market for "eur" and stores what it says; Rust leaves EUR unpriced.
      { name: 'binance-balances-no_rate', setup: install,
        steps: [P.binance_balances(night, account: P.binance_account(%w[EUR 50], %w[BTC 0.1]), rates: P::SERVER_ERROR,
                                   prices: P.prices_body('bitcoin' => 60_000.5, 'eur' => 0.0004))] }
    ]
  end

  # Exchanges::Binance#get_ledger with AccountTransactionSync, both sides served from one Binance account (FakeBinance).
  # Requests differ by design (Rust pages trades by id, walks every source in windows, paces, and reads no margin or
  # futures); the rows are compared, and Rust's reads-more rulings are listed.
  def binance_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    symbols = [%w[BTCUSDT BTC USDT], %w[ETHUSDT ETH USDT], %w[ETHBTC ETH BTC], %w[BTCEUR BTC EUR], %w[BNBUSDT BNB USDT], %w[XRPUSDT XRP USDT BREAK]]
    t = ->(*args) { Time.utc(*args) }
    btc = [P.bn_trade(100, 10, '60000.00', '0.01000000', 1_785_924_000_123, buyer: true, commission: '0.00001000', asset: 'BTC'),
           P.bn_trade(101, 11, '61000.50', '0.00500000', 1_786_010_400_987, buyer: false, commission: '0.30500000', asset: 'USDT')]
    eth = [P.bn_trade(7, 3, '0.04000000', '0.50000000', 1_786_096_800_500, buyer: true, commission: '0.00050000', asset: 'ETH')]
    basic = {
      'symbols' => symbols, 'balances' => [%w[BTC 0.04], %w[USDT 400], %w[ETH 0.4995]],
      'deposits' => [P.bn_deposit(1, 'USDT', '1000', 1_785_578_400_123), P.bn_deposit(2, 'BTC', '0.1', t.(2026, 8, 2, 9)),
                     P.bn_deposit(3, 'USDT', '50', t.(2026, 8, 3), status: 2)],
      'withdrawals' => [P.bn_withdrawal(1, 'BTC', '0.05', t.(2026, 8, 10, 12)), P.bn_withdrawal(2, 'BTC', '0.01', t.(2026, 8, 11), status: 3),
                        P.bn_withdrawal(3, 'USDT', '10', t.(2026, 8, 12), fee: nil)],
      'trades' => { 'BTCUSDT' => btc, 'ETHBTC' => eth }
    }
    incremental = lambda do |watermark, &rows|
      lambda do |ctx|
        P.binance(ctx)
        rows&.call(ctx)
        ctx[:key].update_columns(last_synced_at: watermark)
      end
    end
    stored = lambda do |ctx|
      P.stored(ctx, exchange: ctx[:binance], entry_type: 0, base_currency: 'BTC', base_amount: '0.01', quote_currency: 'USDT', quote_amount: '600',
                    fee_currency: 'BTC', fee_amount: '0.00001', tx_id: 'BTCUSDT-10-100', transacted_at: Time.at(1_785_924_000_123 / 1000.0).utc)
      P.stored(ctx, exchange: ctx[:binance], entry_type: 2, base_currency: 'ETH', base_amount: '0.5', fee_currency: 'ETH', fee_amount: '0.0005',
                    tx_id: 'ETHBTC-3-7-in', group_id: 'swap_ETHBTC-3-7', transacted_at: Time.at(1_786_096_800_500 / 1000.0).utc)
      P.stored(ctx, exchange: ctx[:binance], entry_type: 3, base_currency: 'BTC', base_amount: '0.02',
                    tx_id: 'ETHBTC-3-7-out', group_id: 'swap_ETHBTC-3-7', transacted_at: Time.at(1_786_096_800_500 / 1000.0).utc)
    end
    later = { 'symbols' => symbols, 'balances' => [%w[BTC 0.04], %w[USDT 400], %w[ETH 0.7]],
              'trades' => { 'BTCUSDT' => btc.first(1) + [P.bn_trade(102, 12, '62000', '0.001', t.(2026, 9, 19, 21), buyer: true, asset: 'BNB', commission: '0.0001')],
                            'ETHBTC' => eth + [P.bn_trade(8, 4, '0.041', '0.2', t.(2026, 9, 19, 20, 30), buyer: true, commission: '0.0002', asset: 'ETH')] } }
    newer_trade = ->(id, order, at) { P.bn_trade(id, order, '62000', '0.001', at, buyer: true, asset: 'BNB', commission: '0.0001') }
    stored_trade = lambda do |ctx, id, order, at|
      P.stored(ctx, exchange: ctx[:binance], entry_type: 0, base_currency: 'BTC', base_amount: '0.001', quote_currency: 'USDT', quote_amount: '62',
                    fee_currency: 'BNB', fee_amount: '0.0001', tx_id: "BTCUSDT-#{order}-#{id}", transacted_at: at)
    end
    # The sources after the trades: converts, fiat payments, dust, dividends and Simple Earn, in the shapes Binance
    # documents. Rails' fields everywhere here; the earn rows Rails misreads are their own scenario below.
    at = ->(*args) { P.ms(Time.utc(*args)) }
    others = {
      'converts' => [{ 'quoteId' => 'f3b9e1c2a7d4', 'orderId' => 940_708_407_462_087_195, 'orderStatus' => 'SUCCESS', 'fromAsset' => 'USDT', 'fromAmount' => '100',
                       'toAsset' => 'BNB', 'toAmount' => '0.25000000', 'ratio' => '0.0025', 'inverseRatio' => '400', 'createTime' => 1_786_789_800_321 }],
      'fiat_buys' => [{ 'orderNo' => '353fca443f06466db0c4dc89f94f027a', 'sourceAmount' => '200.0', 'fiatCurrency' => 'EUR', 'obtainAmount' => '0.003',
                        'cryptoCurrency' => 'BTC', 'totalFee' => '2.5', 'price' => '66000', 'status' => 'Completed', 'paymentMethod' => 'Credit Card',
                        'createTime' => at.(2026, 8, 16, 9), 'updateTime' => at.(2026, 8, 16, 9, 1) },
                      { 'orderNo' => 'pending01', 'sourceAmount' => '50.0', 'fiatCurrency' => 'EUR', 'obtainAmount' => '0.0007', 'cryptoCurrency' => 'BTC',
                        'totalFee' => '1', 'price' => '66000', 'status' => 'Processing', 'paymentMethod' => 'Credit Card', 'createTime' => at.(2026, 8, 16, 10),
                        'updateTime' => at.(2026, 8, 16, 10) }],
      'fiat_sells' => [{ 'orderNo' => 'sell01', 'sourceAmount' => '0.001', 'fiatCurrency' => 'EUR', 'obtainAmount' => '60.0', 'cryptoCurrency' => 'BTC',
                         'totalFee' => '0', 'price' => '60000', 'status' => 'Completed', 'createTime' => at.(2026, 8, 17, 9), 'updateTime' => at.(2026, 8, 17, 9) }],
      'dust' => [{ 'operateTime' => 1_786_960_800_777, 'totalTransferedAmount' => '0.000882', 'totalServiceChargeAmount' => '0.000018', 'transId' => 45_178_372_831,
                   'userAssetDribbletDetails' => [
                     { 'transId' => 4_359_321, 'serviceChargeAmount' => '0.000009', 'amount' => '0.9', 'operateTime' => 1_786_960_800_777, 'transferedAmount' => '0.000441',
                       'fromAsset' => 'USDT', 'targetAsset' => 'BNB' },
                     { 'transId' => 4_359_322, 'serviceChargeAmount' => '0.000009', 'amount' => '1.5', 'operateTime' => 1_786_960_800_777, 'transferedAmount' => '0.000441',
                       'fromAsset' => 'XRP', 'targetAsset' => 'BNB' }] }],
      'dividends' => [{ 'id' => 1_637_366_104, 'amount' => '10.00000000', 'asset' => 'BNB', 'divTime' => at.(2026, 8, 18, 12), 'enInfo' => 'BNB Vault',
                        'tranId' => 2_968_885_920, 'direction' => 1 }]
    }
    everything = basic.merge(others)
    file_row = lambda do |ctx, type, coin, amount, at|
      operation = type == 4 ? 'Deposit' : 'Withdraw'
      P.stored(ctx, exchange: ctx[:binance], entry_type: type, base_currency: coin, base_amount: amount, tx_id: nil, transacted_at: at,
                    description: operation, raw_data: { 'operation' => operation })
    end
    fillers = Array.new(99) { |i| P.bn_deposit(100 + i, 'USDT', "#{i + 1}.25", t.(2026, 8, 20) + (i * 60)) }
    second = everything.merge('trades' => { 'BTCUSDT' => btc + [P.bn_trade(102, 12, '62000', '0.001', t.(2026, 9, 19, 21), buyer: true, asset: 'BNB', commission: '0.0001')],
                                           'ETHBTC' => eth },
                              'converts' => others['converts'] + [others['converts'][0].merge('quoteId' => 'a1b2c3', 'fromAmount' => '40', 'toAmount' => '0.1',
                                                                                               'createTime' => at.(2026, 9, 21, 12))],
                              'dividends' => others['dividends'] + [others['dividends'][0].merge('id' => 1_637_366_105, 'amount' => '2.5', 'divTime' => at.(2026, 9, 21))])
    # Binance refuses flexible rewards without a `type` (-1102), which Rails never sends.
    flexible = basic.merge('earn' => { 'flexible_rewardsRecord' => [
      { 'asset' => 'USDT', 'rewards' => '0.05000000', 'projectId' => 'USDT001', 'type' => 'REALTIME', 'time' => at.(2026, 8, 19) },
      { 'asset' => 'USDT', 'rewards' => '0.01000000', 'projectId' => 'USDT001', 'type' => 'BONUS', 'time' => at.(2026, 8, 19, 1) },
      { 'asset' => 'BNB', 'rewards' => '0.00200000', 'projectId' => 'BNB001', 'type' => 'REWARDS', 'time' => at.(2026, 9, 3) }] })
    documented = basic.merge('earn' => {
      'locked_rewardsRecord' => [{ 'positionId' => '123123', 'time' => at.(2026, 8, 20), 'asset' => 'BNB', 'lockPeriod' => '30', 'amount' => '0.00120069', 'type' => 'Locked Rewards' }],
      'flexible_subscriptionRecord' => [{ 'amount' => '100.00000000', 'asset' => 'USDT', 'time' => at.(2026, 8, 21), 'purchaseId' => 26_055, 'productId' => 'USDT001',
                                          'type' => 'AUTO', 'sourceAccount' => 'SPOT', 'amtFromSpot' => '100', 'amtFromFunding' => '0', 'status' => 'SUCCESS' }],
      'flexible_redemptionRecord' => [{ 'amount' => '10.54000000', 'asset' => 'USDT', 'time' => at.(2026, 8, 22), 'productId' => 'USDT001', 'redeemId' => 40_607,
                                        'destAccount' => 'SPOT', 'status' => 'PAID' }],
      'locked_subscriptionRecord' => [{ 'positionId' => '123124', 'purchaseId' => '26056', 'projectId' => 'BNB30', 'time' => at.(2026, 8, 23), 'asset' => 'BNB',
                                        'amount' => '0.5', 'lockPeriod' => '30', 'type' => 'NORMAL', 'sourceAccount' => 'SPOT', 'status' => 'SUCCESS' }],
      'locked_redemptionRecord' => [{ 'positionId' => '123124', 'redeemId' => '40608', 'time' => at.(2026, 9, 2), 'asset' => 'BNB', 'lockPeriod' => '30', 'amount' => '0.5',
                                      'type' => 'MATURE', 'deliverDate' => at.(2026, 9, 2).to_s, 'status' => 'PAID' }]
    })
    [
      { name: 'binance-ledger-other_sources', setup: ->(ctx) { P.binance(ctx) }, steps: [P.binance_ledger(night, everything)] },
      # Two nights over every source; Rust's second night also runs on Rails' first (rust/tests/sync_parity.rs hand-over).
      { name: 'binance-ledger-two_nights', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, everything), P.binance_ledger('2026-09-22T02:00:00.250000Z', second)] },
      { name: 'binance-ledger-flexible_rewards_type', setup: ->(ctx) { P.binance(ctx) }, steps: [P.binance_ledger(night, flexible)] },
      { name: 'binance-ledger-earn_documented_amounts', setup: ->(ctx) { P.binance(ctx) }, steps: [P.binance_ledger(night, documented)] },
      { name: 'binance-ledger-earn_fails', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, documented.merge('fail' => { '/sapi/v1/simple-earn/locked/history/rewardsRecord' => { 'status' => 400, 'body' => { 'code' => -1003, 'msg' => 'Too many requests.' } } }))] },
      { name: 'binance-ledger-full', setup: ->(ctx) { P.binance(ctx) }, steps: [P.binance_ledger(night, basic)] },
      { name: 'binance-ledger-idle', setup: incremental.call(t.(2026, 9, 19, 2)) { |ctx| stored.call(ctx) },
        steps: [P.binance_ledger(night, later.merge('trades' => { 'BTCUSDT' => btc.first(1), 'ETHBTC' => eth }))] },
      { name: 'binance-ledger-deposit_fails', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, basic.merge('fail' => { '/sapi/v1/capital/deposit/hisrec' => { 'status' => 500, 'body' => { 'code' => -1000, 'msg' => 'An unknown error occurred while processing the request.' } } }))] },
      # The watermark counts the trades Rails re-reads: from the floored start, 500 per pair, those already stored too.
      { name: 'binance-ledger-stored_trade_newer', setup: incremental.call(t.(2026, 9, 10)) { |ctx| stored_trade.call(ctx, 102, 12, t.(2026, 9, 15)) },
        steps: [P.binance_ledger(night, { 'symbols' => symbols, 'balances' => [%w[BTC 1]], 'deposits' => [P.bn_deposit(11, 'BTC', '1', t.(2026, 9, 12))],
                                          'trades' => { 'BTCUSDT' => [newer_trade.call(102, 12, t.(2026, 9, 15))] } })] },
      { name: 'binance-ledger-stored_trades_over_500',
        setup: incremental.call(t.(2026, 9, 10)) { |ctx| (1..501).each { |i| stored_trade.call(ctx, i, i, t.(2026, 9, 15) + (i * 60)) } },
        steps: [P.binance_ledger(night, { 'symbols' => symbols, 'balances' => [%w[BTC 1]],
                                          'trades' => { 'BTCUSDT' => (1..501).map { |i| newer_trade.call(i, i, t.(2026, 9, 15) + (i * 60)) } } })] },
      { name: 'binance-ledger-stored_trade_before_floor', setup: incremental.call(t.(2026, 6, 1)) { |ctx| stored_trade.call(ctx, 102, 12, t.(2026, 6, 15)) },
        steps: [P.binance_ledger(night, { 'symbols' => symbols, 'balances' => [%w[BTC 1]], 'trades' => { 'BTCUSDT' => [newer_trade.call(102, 12, t.(2026, 6, 15))] } })] },
      # ---- listed divergences: Rust reads what Rails misses (D2b rulings Q1-Q3, Q7, Q10, Q14, and the window floor) ----
      { name: 'binance-ledger-crypto_pair_later', setup: incremental.call(t.(2026, 9, 19, 2)) { |ctx| stored.call(ctx) }, steps: [P.binance_ledger(night, later)] },
      { name: 'binance-ledger-over_500_trades', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, basic.merge('trades' => { 'BTCUSDT' => Array.new(501) { |i| P.bn_trade(i + 1, i + 1, '60000', '0.0001', 1_785_000_000_000 + (i * 60_000), buyer: i.even?) } }))] },
      { name: 'binance-ledger-deposit_credited', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, basic.merge('deposits' => [P.bn_deposit(9, 'BTC', '0.3', t.(2026, 9, 1), status: 6)]))] },
      { name: 'binance-ledger-pending_withdrawal', setup: incremental.call(t.(2026, 9, 14)),
        steps: [P.binance_ledger(night, { 'symbols' => symbols, 'balances' => [%w[BTC 1]], 'deposits' => [P.bn_deposit(4, 'BTC', '1', t.(2026, 9, 18))],
                                          'withdrawals' => [P.bn_withdrawal(5, 'BTC', '0.2', t.(2026, 9, 15, 10), status: 4)] }),
                P.binance_ledger('2026-09-25T02:00:00.250000Z', { 'symbols' => symbols, 'balances' => [%w[BTC 0.8]], 'deposits' => [P.bn_deposit(4, 'BTC', '1', t.(2026, 9, 18))],
                                                                    'withdrawals' => [P.bn_withdrawal(5, 'BTC', '0.2', t.(2026, 9, 15, 10))] })] },
      { name: 'binance-ledger-symbol_fails', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, basic.merge('fail' => { '/api/v3/myTrades?symbol=ETHBTC' => { 'status' => 429, 'body' => { 'code' => -1003, 'msg' => 'Too many requests.' } } }))] },
      { name: 'binance-ledger-away_100_days', setup: incremental.call(t.(2026, 6, 1)),
        steps: [P.binance_ledger(night, { 'symbols' => symbols, 'balances' => [%w[BTC 1]], 'deposits' => [P.bn_deposit(6, 'BTC', '1', t.(2026, 6, 20))] })] },
      { name: 'binance-ledger-garbage_amount', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, basic.merge('deposits' => [P.bn_deposit(7, 'BTC', 'abc', t.(2026, 9, 1))]))] },
      # Binance.US (plan VB slice A2): the same walk at api.binance.us over every source served; Exchanges::BinanceUs asks only
      # deposits, withdrawals, trades and dividends.
      { name: 'binance-ledger-us_every_source', setup: ->(ctx) { P.binance(ctx, us: true) }, steps: [P.binance_ledger(night, everything)] },
      { name: 'binance-ledger-us_two_nights', setup: ->(ctx) { P.binance(ctx, us: true) },
        steps: [P.binance_ledger(night, everything), P.binance_ledger('2026-09-22T02:00:00.250000Z', second)] },
      # ---- a file imported before the sync: the export's copy of a transfer (credited later; gross at completion) ----
      { name: 'binance-ledger-file_transfers',
        setup: lambda do |ctx|
          P.binance(ctx)
          file_row.call(ctx, 4, 'USDT', '1000', Time.at(1_785_578_400_123 / 1000.0).utc + 46) # credited 46 s after it was seen
          file_row.call(ctx, 4, 'BTC', '0.1', t.(2026, 8, 2, 9, 1))                           # two candidates: a visible double
          file_row.call(ctx, 4, 'BTC', '0.1', t.(2026, 8, 2, 9, 2))
          file_row.call(ctx, 5, 'BTC', '0.0505', t.(2026, 8, 10, 12, 12))                     # gross at completeTime
          file_row.call(ctx, 5, 'USDT', '10', t.(2026, 8, 12, 0, 5))                          # no fee: equal at completeTime
          file_row.call(ctx, 4, 'ETH', '2', t.(2026, 8, 4, 10, 11))                           # eleven minutes on: its own row
        end,
        steps: [P.binance_ledger(night, basic.merge(
          'deposits' => basic['deposits'] + [P.bn_deposit(8, 'ETH', '2', t.(2026, 8, 4, 10))],
          'withdrawals' => [P.bn_withdrawal(1, 'BTC', '0.05', t.(2026, 8, 10, 12)).merge('completeTime' => '2026-08-10 12:12:00'),
                            P.bn_withdrawal(3, 'USDT', '10', t.(2026, 8, 12), fee: nil).merge('completeTime' => '2026-08-12 00:05:00')]
        ))] },
      # One file row, two equal API deposits: the first claims it, across the store's 100-row chunks and across nights.
      { name: 'binance-ledger-file_transfer_claimed_once',
        setup: lambda do |ctx|
          P.binance(ctx)
          file_row.call(ctx, 4, 'USDT', '777', t.(2026, 9, 1, 12, 3))
          file_row.call(ctx, 4, 'USDT', '888', t.(2026, 9, 2, 12, 3))
        end,
        steps: [P.binance_ledger(night, basic.merge('deposits' => fillers + [P.bn_deposit(200, 'USDT', '777', t.(2026, 9, 1, 12)),
                                                                             P.bn_deposit(201, 'USDT', '777', t.(2026, 9, 1, 12, 1)),
                                                                             P.bn_deposit(202, 'USDT', '888', t.(2026, 9, 2, 12))])),
                P.binance_ledger('2026-09-22T02:00:00.250000Z', basic.merge('deposits' => [P.bn_deposit(202, 'USDT', '888', t.(2026, 9, 2, 12)),
                                                                                           P.bn_deposit(203, 'USDT', '888', t.(2026, 9, 2, 12, 1))]))] },
      # The API cuts a Convert to the whole second, the export rounds it up: a file row exactly a second on is the API's.
      # Two id-less rows a second apart are two, whichever side they came from.
      { name: 'binance-ledger-file_second_after',
        setup: lambda do |ctx|
          P.binance(ctx)
          P.stored(ctx, exchange: ctx[:binance], entry_type: 3, base_currency: 'USDT', base_amount: '12.5', tx_id: nil, transacted_at: t.(2026, 8, 13, 14, 29, 19))
          P.stored(ctx, exchange: ctx[:binance], entry_type: 2, base_currency: 'BTC', base_amount: '0.0002', tx_id: nil, transacted_at: t.(2026, 8, 13, 14, 29, 19))
          P.stored(ctx, exchange: ctx[:binance], entry_type: 4, base_currency: 'XRP', base_amount: '30', tx_id: nil, transacted_at: t.(2026, 8, 14, 10, 0, 1))
        end,
        steps: [P.binance_ledger(night, basic.merge(
          'converts' => [others['converts'][0].merge('quoteId' => 'sec1', 'fromAmount' => '12.5', 'toAsset' => 'BTC', 'toAmount' => '0.0002',
                                                     'createTime' => at.(2026, 8, 13, 14, 29, 18))],
          'deposits' => basic['deposits'] + [P.bn_deposit(9, 'XRP', '30', t.(2026, 8, 14, 10), tx: nil)]
        ))] },
      # Credited in the export exactly ten minutes after the API saw it: the same deposit.
      { name: 'binance-ledger-deposit_600',
        setup: lambda do |ctx|
          P.binance(ctx)
          file_row.call(ctx, 4, 'ETH', '2', t.(2026, 8, 4, 10, 10))
        end,
        steps: [P.binance_ledger(night, basic.merge('deposits' => basic['deposits'] + [P.bn_deposit(8, 'ETH', '2', t.(2026, 8, 4, 10))]))] },
      # Two id-less deposits alike in one read are two deposits: a row this run stored is not evidence against the next.
      { name: 'binance-ledger-idless_repeats', setup: ->(ctx) { P.binance(ctx) },
        steps: [P.binance_ledger(night, basic.merge('deposits' => basic['deposits'] + [P.bn_deposit(10, 'XRP', '30', t.(2026, 8, 14, 10), tx: nil),
                                                                                        P.bn_deposit(11, 'XRP', '30', t.(2026, 8, 14, 10), tx: nil)]))] },
      # Rows stored before the run count one for one: three alike over two stored adds one, and two dividends over one
      # file row add one.
      { name: 'binance-ledger-preexisting_repeats',
        setup: lambda do |ctx|
          P.binance(ctx)
          2.times { P.stored(ctx, exchange: ctx[:binance], entry_type: 4, base_currency: 'XRP', base_amount: '30', tx_id: nil, transacted_at: t.(2026, 8, 14, 10)) }
          P.stored(ctx, exchange: ctx[:binance], entry_type: 8, base_currency: 'BNB', base_amount: '10', tx_id: nil, transacted_at: t.(2026, 8, 18, 12))
        end,
        steps: [P.binance_ledger(night, basic.merge(
          'deposits' => basic['deposits'] + Array.new(3) { |i| P.bn_deposit(10 + i, 'XRP', '30', t.(2026, 8, 14, 10), tx: nil) },
          'dividends' => [others['dividends'][0].merge('amount' => '10'), others['dividends'][0].merge('id' => 1_637_366_199, 'amount' => '10')]
        ))] },
      # A deposit whose id a file row took (dated when credited), and its id-less twin stored too: the id'd entry is
      # skipped on its id and does not use up the id-less entry's match.
      { name: 'binance-ledger-known_id_repeat',
        setup: lambda do |ctx|
          P.binance(ctx)
          P.stored(ctx, exchange: ctx[:binance], entry_type: 4, base_currency: 'XRP', base_amount: '30', tx_id: '0xd12', transacted_at: t.(2026, 8, 14, 10, 0, 46),
                        raw_data: { 'operation' => 'Deposit' })
          P.stored(ctx, exchange: ctx[:binance], entry_type: 4, base_currency: 'XRP', base_amount: '30', tx_id: nil, transacted_at: t.(2026, 8, 14, 10))
        end,
        steps: [P.binance_ledger(night, basic.merge('deposits' => basic['deposits'] + [P.bn_deposit(12, 'XRP', '30', t.(2026, 8, 14, 10)),
                                                                                        P.bn_deposit(13, 'XRP', '30', t.(2026, 8, 14, 10), tx: nil)]))] },
      # The export's copy of a dust sweep: the BNB net of the charge, a second after operateTime. A whole conversion pairs,
      # an amount the column rounds too (compared as stored); a group holding one leg of it pairs with nothing.
      { name: 'binance-ledger-file_dust',
        setup: lambda do |ctx|
          P.binance(ctx)
          sweep = Time.at(1_786_960_801).utc # operateTime 1_786_960_800_777, written a second on
          [[3, 'USDT', '0.9', 'USDT'], [2, 'BNB', '0.000432', 'USDT'], [3, 'XRP', '1.5', 'XRP'],
           [3, 'SHIB', '123456789.12345678', 'SHIB'], [2, 'BNB', '0.00005', 'SHIB']].each do |type, coin, amount, from|
            P.stored(ctx, exchange: ctx[:binance], entry_type: type, base_currency: coin, base_amount: amount, tx_id: nil, transacted_at: sweep,
                          group_id: "dustcsv_#{from} to BNB_#{sweep.to_i}")
          end
        end,
        steps: [P.binance_ledger(night, basic.merge('dust' => others['dust'].map do |sweep|
          shib = { 'transId' => 4_359_323, 'serviceChargeAmount' => '0.000001', 'amount' => '123456789.12345678',
                   'operateTime' => sweep['operateTime'],
                   'transferedAmount' => '0.000051', 'fromAsset' => 'SHIB', 'targetAsset' => 'BNB' }
          sweep.merge('userAssetDribbletDetails' => sweep['userAssetDribbletDetails'] + [shib])
        end))] },
      # The export rounds the net BNB on its own: a leg one unit of the 8th decimal off either way pairs, two units off
      # pairs with nothing.
      { name: 'binance-ledger-file_dust_rounding',
        setup: lambda do |ctx|
          P.binance(ctx)
          sweep = Time.at(1_786_960_801).utc
          [%w[ADA 0.0031 0.00001022], %w[EUR 5.2 0.01345686], %w[USDT 0.9 0.00043199], %w[XRP 1.5 0.00043202]].each do |coin, amount, net|
            [[3, coin, amount], [2, 'BNB', net]].each do |type, currency, value|
              P.stored(ctx, exchange: ctx[:binance], entry_type: type, base_currency: currency, base_amount: value, tx_id: nil, transacted_at: sweep,
                            group_id: "dustcsv_#{coin} to BNB_#{sweep.to_i}")
            end
          end
        end,
        steps: [P.binance_ledger(night, basic.merge(
          'dust' => [{ 'operateTime' => 1_786_960_800_777, 'totalTransferedAmount' => '0.0151459', 'totalServiceChargeAmount' => '0.00029502', 'transId' => 45_178_372_833,
                       'userAssetDribbletDetails' => [%w[ADA 0.0031 0.00001042 0.00000021], %w[EUR 5.2 0.01373148 0.00027463],
                                                      %w[USDT 0.9 0.000441 0.000009], %w[XRP 1.5 0.000441 0.000009]].each_with_index.map do |(coin, amount, gross, charge), i|
                         { 'transId' => 4_359_340 + i, 'serviceChargeAmount' => charge, 'amount' => amount, 'operateTime' => 1_786_960_800_777,
                           'transferedAmount' => gross, 'fromAsset' => coin, 'targetAsset' => 'BNB' }
                       end }]
        ))] },
      # A stored row no pairing governs, exactly a second on (across sources), is one event: of two identical API dust
      # conversions, or deposits, it accounts for the first only.
      { name: 'binance-ledger-twin_repeats',
        setup: lambda do |ctx|
          P.binance(ctx)
          P.stored(ctx, exchange: ctx[:binance], entry_type: 3, base_currency: 'USDT', base_amount: '0.9', tx_id: nil, transacted_at: Time.at(1_786_960_801).utc)
          P.stored(ctx, exchange: ctx[:binance], entry_type: 4, base_currency: 'XRP', base_amount: '30', tx_id: nil, transacted_at: t.(2026, 8, 14, 10, 0, 1))
        end,
        steps: [P.binance_ledger(night, basic.merge(
          'dust' => [{ 'operateTime' => 1_786_960_800_000, 'totalTransferedAmount' => '0.000882', 'totalServiceChargeAmount' => '0.000018', 'transId' => 45_178_372_832,
                       'userAssetDribbletDetails' => Array.new(2) do |i|
                         { 'transId' => 4_359_330 + i, 'serviceChargeAmount' => '0.000009', 'amount' => '0.9', 'operateTime' => 1_786_960_800_000,
                           'transferedAmount' => '0.000441', 'fromAsset' => 'USDT', 'targetAsset' => 'BNB' }
                       end }],
          'deposits' => basic['deposits'] + [P.bn_deposit(20, 'XRP', '30', t.(2026, 8, 14, 10)), P.bn_deposit(21, 'XRP', '30', t.(2026, 8, 14, 10))]
        ))] }
    ]
  end

  # The tracker's import (Import::Run#import!, after the controller's zone and chosen-key steps), alone and beside the
  # Binance API sync over the same history in either order. Every file is SYNTHETIC: User ID 1, made-up amounts.
  def import_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    before = '2026-09-19T12:00:00.500000Z'
    after = '2026-09-20T03:00:00.500000Z'
    t = ->(*args) { Time.utc(*args) }
    utc0 = 'Binance-Transaction-History-202609190000(UTC+0)-part1-of1.csv'
    binance_csv = ->(*rows) { (['User ID,Time,Account,Operation,Coin,Change,Remark'] + rows.map { |r| "1,#{r.join(',')}" }).join("\n") }
    ours = ->(*rows) { ([AccountTransaction.csv_headers.join(',')] + rows.map { |r| r.join(',') }).join("\n") }
    state = {
      'symbols' => [%w[BTCUSDT BTC USDT], %w[ETHUSDT ETH USDT], %w[BNBUSDT BNB USDT], %w[XRPUSDT XRP USDT]], 'balances' => [%w[BTC 0.05]], 'trades' => {},
      'deposits' => [P.bn_deposit(1, 'USDT', '1000', 1_785_578_400_123), P.bn_deposit(2, 'BTC', '0.1', t.(2026, 8, 2, 9))],
      'withdrawals' => [P.bn_withdrawal(1, 'BTC', '0.05', t.(2026, 8, 10, 12)).merge('completeTime' => '2026-08-10 12:12:00')],
      'dust' => [{ 'operateTime' => 1_786_960_800_777, 'totalTransferedAmount' => '0.000882', 'totalServiceChargeAmount' => '0.000018', 'transId' => 45_178_372_831,
                   'userAssetDribbletDetails' => [
                     { 'transId' => 4_359_321, 'serviceChargeAmount' => '0.000009', 'amount' => '0.9', 'operateTime' => 1_786_960_800_777, 'transferedAmount' => '0.000441',
                       'fromAsset' => 'USDT', 'targetAsset' => 'BNB' },
                     { 'transId' => 4_359_322, 'serviceChargeAmount' => '0.000009', 'amount' => '1.5', 'operateTime' => 1_786_960_800_777, 'transferedAmount' => '0.000441',
                       'fromAsset' => 'XRP', 'targetAsset' => 'BNB' }] }]
    }
    # The export's copy of the same history: deposits as credited (46 s and 3 min on), the withdrawal gross at its
    # completion, the dust sweep a second on with the BNB net of the charge, and a dust credit with no spend (#593).
    transfers = binance_csv.(['2026-08-01 10:00:46', 'Spot', 'Deposit', 'USDT', '1000', ''], ['2026-08-02 09:03:00', 'Spot', 'Deposit', 'BTC', '0.1', ''],
                             ['2026-08-10 12:12:00', 'Spot', 'Withdraw', 'BTC', '-0.0505', 'Withdraw fee is included'])
    dust = binance_csv.(['2026-08-17 10:00:01', 'Spot', 'Small Assets Exchange BNB', 'USDT', '-0.9', 'USDT to BNB'],
                        ['2026-08-17 10:00:01', 'Spot', 'Small Assets Exchange BNB', 'BNB', '0.000432', 'USDT to BNB'],
                        ['2026-08-17 10:00:01', 'Spot', 'Small Assets Exchange BNB', 'XRP', '-1.5', 'XRP to BNB'],
                        ['2026-08-17 10:00:01', 'Spot', 'Small Assets Exchange BNB', 'BNB', '0.000432', 'XRP to BNB'],
                        ['2026-08-18 10:00:00', 'Spot', 'Small Assets Exchange BNB', 'BNB', '0.0001', 'SHIB to BNB'])
    # A sweep the export rounds on its own: its BNB one unit of the 8th decimal off either way pairs; two off do not.
    # Six rows: macOS Ruby's sort_by (the recorder's) is unstable beyond that on rows of one second; the app's Linux
    # image, and Rust, sort stably.
    rounding = [%w[ADA 0.0031 0.00001042 0.00000021 0.00001022], %w[USDT 0.9 0.000441 0.000009 0.00043199], %w[XRP 1.5 0.000441 0.000009 0.00043202]]
    rounding_state = state.merge('dust' => [{ 'operateTime' => 1_786_960_800_777, 'totalTransferedAmount' => '0.00089242', 'totalServiceChargeAmount' => '0.00001821',
                                              'transId' => 45_178_372_833,
                                              'userAssetDribbletDetails' => rounding.each_with_index.map do |(coin, amount, gross, charge, _), i|
                                                { 'transId' => 4_359_340 + i, 'serviceChargeAmount' => charge, 'amount' => amount, 'operateTime' => 1_786_960_800_777,
                                                  'transferedAmount' => gross, 'fromAsset' => coin, 'targetAsset' => 'BNB' }
                                              end }])
    rounded = binance_csv.(*rounding.flat_map do |coin, amount, _, _, net|
      [['2026-08-17 10:00:01', 'Spot', 'Small Assets Exchange BNB', coin, "-#{amount}", "#{coin} to BNB"],
       ['2026-08-17 10:00:01', 'Spot', 'Small Assets Exchange BNB', 'BNB', net, "#{coin} to BNB"]]
    end)
    binance = ->(ctx) { P.binance(ctx) }
    both = lambda do |ctx|
      P.binance(ctx)
      P.binance(ctx, us: true)
    end
    [
      { name: 'import-file_then_api_transfers', setup: binance,
        steps: [P.import(before, 'binance', transfers, filename: utc0), P.binance_ledger(night, state)] },
      { name: 'import-api_then_file_transfers', setup: binance,
        steps: [P.binance_ledger(night, state), P.import(after, 'binance', transfers, filename: utc0)] },
      { name: 'import-file_then_api_dust', setup: binance,
        steps: [P.import(before, 'binance', dust, filename: utc0), P.binance_ledger(night, state)] },
      { name: 'import-api_then_file_dust', setup: binance,
        steps: [P.binance_ledger(night, state), P.import(after, 'binance', dust, filename: utc0)] },
      { name: 'import-file_then_api_dust_rounding', setup: binance,
        steps: [P.import(before, 'binance', rounded, filename: utc0), P.binance_ledger(night, rounding_state)] },
      { name: 'import-api_then_file_dust_rounding', setup: binance,
        steps: [P.binance_ledger(night, rounding_state), P.import(after, 'binance', rounded, filename: utc0)] },
      # The same file twice, and a file whose identical rows are each a real credit, imported twice.
      { name: 'import-same_file_twice', setup: binance,
        steps: [P.import(before, 'binance', transfers, filename: utc0), P.import(after, 'binance', transfers, filename: utc0)] },
      { name: 'import-repeated_rows', setup: binance,
        steps: Array.new(2) do |i|
          P.import(i.zero? ? before : after, 'binance',
                   binance_csv.(*Array.new(3) { ['2026-08-05 10:00:00', 'Spot', 'Commission Rebate', 'BNB', '0.001', ''] }), filename: utc0)
        end },
      # A file a zone ahead: every time read three hours back.
      { name: 'import-zone_ahead', setup: binance,
        steps: [P.import(before, 'binance', transfers, filename: 'Binance-Transaction-History-202609190000(UTC+3)-part1-of1.csv')] },
      # A Convert an older import stored as a purchase (and one as a sale): the pair is skipped whole. One at another
      # amount is imported.
      { name: 'import-legacy_purchase_then_convert',
        setup: lambda do |ctx|
          P.binance(ctx)
          P.stored(ctx, exchange: ctx[:binance], entry_type: 0, base_currency: 'BNB', base_amount: '0.25', quote_currency: 'USDT', quote_amount: '100',
                        tx_id: nil, transacted_at: t.(2026, 8, 15, 10, 30))
          P.stored(ctx, exchange: ctx[:binance], entry_type: 1, base_currency: 'ETH', base_amount: '0.5', quote_currency: 'USDT', quote_amount: '1200',
                        tx_id: nil, transacted_at: t.(2026, 8, 16, 11))
        end,
        steps: [P.import(before, 'binance', binance_csv.(['2026-08-15 10:30:00', 'Spot', 'Binance Convert', 'USDT', '-100', ''],
                                                         ['2026-08-15 10:30:00', 'Spot', 'Binance Convert', 'BNB', '0.25', ''],
                                                         ['2026-08-16 11:00:00', 'Spot', 'Binance Convert', 'ETH', '-0.5', ''],
                                                         ['2026-08-16 11:00:00', 'Spot', 'Binance Convert', 'USDT', '1200', ''],
                                                         ['2026-08-17 11:00:00', 'Spot', 'Binance Convert', 'USDT', '-40', ''],
                                                         ['2026-08-17 11:00:00', 'Spot', 'Binance Convert', 'BNB', '0.1', '']), filename: utc0)] },
      { name: 'import-legacy_token_swap',
        setup: lambda do |ctx|
          P.binance(ctx)
          P.stored(ctx, exchange: ctx[:binance], entry_type: 11, base_currency: 'NEWT', base_amount: '12', tx_id: nil, transacted_at: t.(2026, 8, 20, 8),
                        raw_data: { 'operation' => 'Token Swap - Distribution' })
        end,
        steps: [P.import(before, 'binance', binance_csv.(['2026-08-20 08:00:00', 'Spot', 'Token Swap - Distribution', 'NEWT', '12', ''],
                                                         ['2026-08-21 08:00:00', 'Spot', 'Token Swap - Distribution', 'NEWT', '3', '']), filename: utc0)] },
      # A row's id names a bot order on its own venue only (#591): the same id on Alpaca is another order.
      { name: 'import-bot_order_by_tx_id',
        setup: lambda do |ctx|
          P.binance(ctx)
          P.holder(ctx, 'AAPL', at: t.(2026, 8, 5, 9), external_id: 'order-591')
          P.holder(ctx, 'AAPL', at: t.(2026, 8, 5, 9), external_id: 'order-591', venue: ctx[:binance])
        end,
        steps: [P.import(before, 'deltabadger', ours.(['2026-08-05T10:00:00Z', 'buy', 'BTC', '0.01', 'USDT', '600', '', '', 'binance', 'order-591', '', '']))] },
      # Our own export across three venues: each row finds its account; the venue nothing is connected to is reported.
      { name: 'import-deltabadger_three_venues', setup: binance,
        steps: [P.import(before, 'deltabadger', ours.(['2026-08-05T10:00:00Z', 'deposit', 'USDT', '5', '', '', '', '', 'binance', '', '', 'from a friend'],
                                                     ['2026-08-06T15:30:00Z', 'buy', 'AAPL', '1', 'USD', '150', 'USD', '0', 'alpaca', 'fill-1', '', ''],
                                                     ['2026-08-07T10:00:00Z', 'deposit', 'EUR', '10', '', '', '', '', 'kraken', '', '', ''],
                                                     ['2026-08-08T10:00:00Z', 'deposit', 'EUR', '10', '', '', '', '', '', '', '', ''],
                                                     ['2026-08-09T10:00:00Z', 'teleport', 'EUR', '10', '', '', '', '', 'binance', '', '', '']))] },
      # Binance and Binance.US write the same export: the file is parked until the user says which; an answer naming
      # another venue's key is no answer.
      { name: 'import-binance_us_ambiguous', setup: both,
        steps: [P.import(before, 'binance', transfers, filename: utc0),
                P.import(before, 'binance', transfers, filename: utc0, answer: 'Exchanges::Alpaca'),
                P.import(after, 'binance', transfers, filename: utc0, answer: 'Exchanges::BinanceUs')] },
      { name: 'import-no_account', steps: [P.import(before, 'binance', transfers, filename: utc0)] },
      { name: 'import-unknown_timezone', setup: binance, steps: [P.import(before, 'binance', transfers, filename: 'export.csv')] },
      { name: 'import-unreadable', setup: binance, steps: [P.import(before, 'binance', binance_csv.(), filename: utc0)] },
      { name: 'import-malformed', setup: binance, steps: [P.import(before, 'binance', "#{binance_csv.()}\n1,2026-08-01 10:00:00,Spot,Deposit,US\"DT,1,", filename: utc0)] },
      # LUNA before Terra collapsed is the coin the alias names, on a crypto venue; after it, the listing.
      { name: 'import-luna_alias',
        setup: lambda do |ctx|
          P.binance(ctx)
          luna = Asset.create!(external_id: 'terra-luna-2', symbol: 'LUNA', name: 'Terra', category: 'Cryptocurrency')
          Asset.create!(external_id: 'terra-luna', symbol: 'LUNC', name: 'Terra Luna Classic', category: 'Cryptocurrency')
          ExchangeAsset.create!(exchange: ctx[:binance], asset: luna, available: true)
          Ticker.create!(exchange: ctx[:binance], ticker: 'LUNAUSDT', base: 'LUNA', quote: 'USDT', base_asset: luna, quote_asset: ctx[:assets]['USDT'],
                         base_decimals: 8, quote_decimals: 8, price_decimals: 2, minimum_base_size: '0.01', minimum_quote_size: '5')
        end,
        steps: [P.import(before, 'binance', binance_csv.(['2021-05-01 10:00:00', 'Spot', 'Deposit', 'LUNA', '100', ''],
                                                         ['2023-01-01 10:00:00', 'Spot', 'Deposit', 'LUNA', '5', '']), filename: utc0)] }
    ] + [true, false].map do |terra|
      # A stock venue never reads a crypto alias: IBKR's LUNA in 2021 is the listed security, with Terra in the
      # catalogue or not.
      { name: "import-ibkr_luna#{'_no_terra' unless terra}",
        setup: lambda do |ctx|
          ibkr = Exchanges::Ibkr.create!(name: 'IBKR', maker_fee: 0, taker_fee: 0)
          stock = Asset.create!(external_id: 'LUNA.US', symbol: 'LUNA', name: 'Luna Innovations', category: 'Stock', instrument_type: 'stock')
          Asset.create!(external_id: 'terra-luna', symbol: 'LUNC', name: 'Terra Luna Classic', category: 'Cryptocurrency') if terra
          [stock, ctx[:assets]['USD']].each { |asset| ExchangeAsset.create!(exchange: ibkr, asset:, available: true) }
          Ticker.create!(exchange: ibkr, ticker: 'LUNA', base: 'LUNA', quote: 'USD', base_asset: stock, quote_asset: ctx[:assets]['USD'],
                         base_decimals: 0, quote_decimals: 2, price_decimals: 2, minimum_base_size: '1', minimum_quote_size: '1')
          ApiKey.new(user: ctx[:user], exchange: ibkr, key: 'ibkr-key', secret: 'ibkr-secret', status: ApiKey.statuses.key(1), key_type: :read_only).save!(validate: false)
        end,
        steps: [P.import(before, 'deltabadger', ours.(['2021-03-01T15:00:00Z', 'buy', 'LUNA', '10', 'USD', '50', '', '', 'ibkr', 'ibkr-1', '', '']))] }
    end
  end

  # Exchanges::Kucoin#get_ledger with AccountTransactionSync (plan K6): weekly fills windows (halved past KuCoin's 50,000
  # cap), SUCCESS deposits and withdrawals keyed by id, a throttled page waited out, any other failure failing the run.
  def kucoin_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    t = ->(*args) { Time.utc(*args) }
    since = ->(watermark) { ->(ctx) { P.kucoin(ctx); ctx[:key].update_columns(last_synced_at: watermark) } }
    week = since.(t.(2026, 9, 15))
    fills = [P.kc_fill('9001', 'BTC-USDT', 'buy', '0.001', '60.5', t.(2026, 9, 16, 10)),
             P.kc_fill('9002', 'ETH-BTC', 'sell', '0.5', '0.02', t.(2026, 9, 17, 11), fee: '-0.00002', fee_currency: 'BTC'),
             P.kc_fill('9001', 'ETH-USDT', 'sell', '0.2', '500', 1_789_700_000_123)] # the same tradeId on another symbol: its own row
    deposits = [P.kc_transfer('d1', 'USDT', '1000', t.(2026, 9, 15, 12), fee: '0.5'), P.kc_transfer(nil, 'BTC', '0.01', t.(2026, 9, 16), wallet: '0xidless')]
    withdrawals = [P.kc_transfer('w1', 'BTC', '0.002', t.(2026, 9, 18), fee: '0.0005', wallet: '0xbatch'),
                   P.kc_transfer('w2', 'ETH', '0.1', t.(2026, 9, 18), fee: '0.001', wallet: '0xbatch'),
                   P.kc_transfer('w3', 'USDT', '5', t.(2026, 9, 18, 1), wallet: '')]
    over_cap = P.kc_page([], pages: 101, total: 50_001)
    throttled = { 'status' => 429, 'body' => { 'code' => '429000', 'msg' => 'Too Many Requests' } }
    [
      { name: 'kucoin-ledger-week', setup: week,
        steps: [P.kucoin_ledger(night, fills: [P.kc_page(fills)], deposits: [P.kc_page(deposits)], withdrawals: [P.kc_page(withdrawals)])] },
      { name: 'kucoin-ledger-full', setup: ->(ctx) { P.kucoin(ctx) },
        steps: [P.kucoin_ledger(night, fills: [P.kc_page(fills.first(1)), P.kc_page([])], deposits: [P.kc_page(deposits.first(1))])] },
      { name: 'kucoin-ledger-two_nights', setup: week,
        steps: [P.kucoin_ledger(night, fills: [P.kc_page(fills)]), P.kucoin_ledger('2026-09-21T02:00:00.250000Z', fills: [P.kc_page(fills)], deposits: [P.kc_page(deposits)])] },
      { name: 'kucoin-ledger-pages', setup: week,
        steps: [P.kucoin_ledger(night, fills: [P.kc_page(fills.first(1), pages: 2, total: 2), P.kc_page(fills.last(1), page: 2, pages: 2, total: 2)],
                                       deposits: [P.kc_page(deposits.first(1), pages: 2, total: 2), P.kc_page(deposits.last(1), page: 2, pages: 2, total: 2)])] },
      { name: 'kucoin-ledger-cap_split', setup: week,
        steps: [P.kucoin_ledger(night, fills: [over_cap, P.kc_page(fills.first(1)), P.kc_page(fills.last(1)), P.kc_page([])])] },
      { name: 'kucoin-ledger-cap_at_an_hour', setup: since.(t.(2026, 9, 21, 1, 30)),
        steps: [P.kucoin_ledger(night, fills: [over_cap])] },
      { name: 'kucoin-ledger-throttled', setup: week,
        steps: [P.kucoin_ledger(night, fills: [throttled, throttled, P.kc_page(fills)])] },
      { name: 'kucoin-ledger-throttled_out', setup: week, steps: [P.kucoin_ledger(night, fills: [throttled])] },
      # Two weekly windows: the first read, the second failed. Nothing is stored and the watermark stays.
      { name: 'kucoin-ledger-failed_page', setup: since.(t.(2026, 9, 10)),
        steps: [P.kucoin_ledger(night, fills: [P.kc_page(fills.first(1)), { 'status' => 500, 'body' => { 'code' => '500000', 'msg' => 'Internal Server Error' } }])] },
      { name: 'kucoin-ledger-error_envelope', setup: week,
        steps: [P.kucoin_ledger(night, fills: [P.kc_page(fills)], deposits: [P.ok('code' => '400007', 'msg' => 'Access Denied')])] },
      { name: 'kucoin-ledger-network', setup: week,
        steps: [P.kucoin_ledger(night, withdrawals: [{ 'network' => 'kucoin_pre_send', 'message' => 'Connection refused - connect(2) for "api.kucoin.com" port 443' }])] },
      { name: 'kucoin-ledger-nothing', setup: week, steps: [P.kucoin_ledger(night)] }
    ]
  end

  # Exchanges::Kucoin#get_balances with AccountBalance::Sync (plan K4): the classic trade account, each currency by the
  # venue's tickers, `holds` or (since KuCoin's 2026-08-27 rename) `locked`; only the success code means balances.
  def kucoin_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25, 'tether' => 1.0001)
    install = ->(ctx) { P.kucoin(ctx) }
    seeded = lambda do |ctx|
      P.kucoin(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2', exchange: ctx[:kucoin])
      P.balance(ctx, 'ETH', free: '1', usd_price: '2400', usd_value: '2400', exchange: ctx[:kucoin])
    end
    held = P.kucoin_accounts(%w[BTC 0.5 0.1], %w[ETH 2], %w[USDT 100.5], %w[KCS 3], %w[DOGE 0])
    [
      # Coins by the market's prices; a currency the venue lists no ticker for (KCS) is not a balance; holds count.
      { name: 'kucoin-balances-first', setup: install, steps: [P.kucoin_balances(night, accounts: held, prices: coins)] },
      { name: 'kucoin-balances-locked', setup: install,
        steps: [P.kucoin_balances(night, accounts: P.kucoin_accounts(%w[BTC 0 0.25], %w[ETH 1.5 0.5], locked: true), prices: coins)] },
      # What was held and is gone, and a price the market cannot give.
      { name: 'kucoin-balances-stale_and_gone', setup: seeded, steps: [P.kucoin_balances(night, accounts: P.kucoin_accounts(%w[BTC 0.45]), prices: P::SERVER_ERROR)] },
      { name: 'kucoin-balances-invalid_key', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: { 'status' => 401, 'body' => { 'code' => '400003', 'msg' => 'KC-API-KEY not exists' } })] },
      { name: 'kucoin-balances-invalid_key_envelope', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: P.ok('code' => '400005', 'msg' => 'Signature error'))] },
      { name: 'kucoin-balances-access_denied', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: { 'status' => 400, 'body' => { 'code' => '400007', 'msg' => 'Access Denied' } })] },
      { name: 'kucoin-balances-throttled', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: { 'status' => 429, 'body' => { 'code' => '429000', 'msg' => 'Too Many Requests' } })] },
      { name: 'kucoin-balances-bad_gateway', setup: seeded, steps: [P.kucoin_balances(night, accounts: { 'status' => 502, 'body' => '<html>502 Bad Gateway</html>' })] },
      { name: 'kucoin-balances-not_json', setup: seeded, steps: [P.kucoin_balances(night, accounts: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'kucoin-balances-network', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: { 'network' => 'kucoin_pre_send', 'message' => 'Connection refused - connect(2) for "api.kucoin.com" port 443' })] },
      { name: 'kucoin-balances-no_data', setup: seeded, steps: [P.kucoin_balances(night, accounts: P.ok('code' => '200000'))] },
      { name: 'kucoin-balances-error_with_data', setup: seeded, steps: [P.kucoin_balances(night, accounts: P.ok('code' => '400100', 'msg' => 'nope', 'data' => []))] },
      { name: 'kucoin-balances-no_currency', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: P.ok('code' => '200000', 'data' => [{ 'available' => '1', 'holds' => '0' }]))] },
      { name: 'kucoin-balances-no_available', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: P.ok('code' => '200000', 'data' => [{ 'currency' => 'BTC', 'holds' => '0' }]))] },
      { name: 'kucoin-balances-no_holds', setup: seeded,
        steps: [P.kucoin_balances(night, accounts: P.ok('code' => '200000', 'data' => [{ 'currency' => 'BTC', 'available' => '1' }]))] },
      { name: 'kucoin-balances-nothing', setup: seeded, steps: [P.kucoin_balances(night, accounts: P.kucoin_accounts)] }
    ]
  end

  # Exchanges::Bitvavo#get_ledger with AccountTransactionSync (plan VB slice A3, AUDIT-bitvavo V5/V6): deposits and withdrawals
  # walked back from now by `end` with the boundary millisecond read again and identical rows counted, then the markets the
  # transfers, the balance and the stored rows name, then their trades by `tradeIdTo`; any failed read failing the run.
  def bitvavo_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    t = ->(*args) { Time.utc(*args) }
    since = ->(watermark) { ->(ctx) { P.bitvavo(ctx); ctx[:key].update_columns(last_synced_at: watermark) } }
    week = since.(t.(2026, 9, 15))
    deposits = [P.bv_transfer('EUR', '1000', t.(2026, 9, 18, 12), tx: nil), P.bv_transfer('BTC', '0.01', t.(2026, 9, 17), fee: '0', tx: '0xd2').except('status'),
                P.bv_transfer('ETH', '2', t.(2026, 9, 16), status: 'pending', tx: '0xd3'), P.bv_transfer('EUR', '50', t.(2026, 9, 16, 1), fee: '0.5')]
    withdrawals = [P.bv_transfer('BTC', '0.002', t.(2026, 9, 19), fee: '0.0001', tx: '0xw1'),
                   P.bv_transfer('ETH', '0.1', t.(2026, 9, 18), fee: '0.001', status: 'awaiting_processing', tx: '0xw2')]
    trades = [P.bv_trade('tr-3', 'BTC-EUR', 'sell', '0.001', '64100', t.(2026, 9, 19, 10)),
              P.bv_trade('tr-2', 'BTC-EUR', 'buy', '0.002', '64000', t.(2026, 9, 18, 9), fee: '-0.01'),
              P.bv_trade('tr-1', 'BTC-EUR', 'buy', '0.003', '63000', t.(2026, 9, 10, 9))] # before the window: read, never stored
    markets = P.bv_markets('BTC-EUR', 'ETH-EUR', 'ETH-BTC', 'ADA-EUR')
    # Pages of 1000: Bitvavo's limit. `boundary`: the oldest millisecond of a full page read again on the next one, with an
    # equal pair in it (two identical 5 EUR deposits) that must stay two.
    base = t.(2026, 9, 1).to_i * 1000
    full = Array.new(998) { |i| P.bv_transfer('EUR', format('1.%03d', i), base + ((1000 - i) * 60_000)) }
    pair = P.bv_transfer('EUR', '5', base + (2 * 60_000))
    page1 = full + [pair, pair]
    page2 = [pair, pair, P.bv_transfer('BTC', '0.5', base + 60_000)]
    trade_page = Array.new(1000) { |i| P.bv_trade("tp-#{1000 - i}", 'BTC-EUR', 'buy', '0.0001', '64000', t.(2026, 9, 19).to_i * 1000 - (i * 1000)) }
    [
      { name: 'bitvavo-ledger-week', setup: week,
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(deposits)], withdrawals: [P.ok(withdrawals)], balance: [P.bv_balance(%w[BTC 0.5], %w[ADA 0 0], %w[EUR 10])],
                                 markets: [markets], trades: [P.ok(trades)])] },
      { name: 'bitvavo-ledger-full', setup: ->(ctx) { P.bitvavo(ctx) },
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(deposits.first(2))], markets: [markets], trades: [P.ok(trades)])] },
      { name: 'bitvavo-ledger-two_nights', setup: week,
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(deposits)], markets: [markets], trades: [P.ok(trades)]),
                P.bitvavo_ledger('2026-09-21T02:00:00.250000Z', deposits: [P.ok(deposits)], withdrawals: [P.ok(withdrawals)], markets: [markets],
                                 trades: [P.ok(trades), P.ok([])])] },
      # A coin held only in an order, and one traded before (a stored sale), name their markets too.
      { name: 'bitvavo-ledger-discovery',
        setup: lambda do |ctx|
          week.(ctx)
          AccountTransaction.create!(user: ctx[:user], api_key: ctx[:key], exchange: ctx[:bitvavo], entry_type: :sell, base_currency: 'ETH', base_amount: 1,
                                     quote_currency: 'BTC', quote_amount: 0.03, tx_id: 'old-1', transacted_at: t.(2026, 8, 1), raw_data: {})
        end,
        steps: [P.bitvavo_ledger(night, balance: [P.bv_balance(%w[ADA 0 7])], markets: [markets],
                                 trades: [P.ok([P.bv_trade('e-1', 'ETH-BTC', 'buy', '1', '0.031', t.(2026, 9, 17), fee_currency: 'BTC', fee: '0.00001')]), P.ok([])])] },
      { name: 'bitvavo-ledger-boundary', setup: ->(ctx) { P.bitvavo(ctx) },
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(page1), P.ok(page2)], markets: [P.bv_markets('ETH-EUR')])] },
      { name: 'bitvavo-ledger-nothing_new', setup: ->(ctx) { P.bitvavo(ctx) },
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(page1), P.ok(page1)])] },
      { name: 'bitvavo-ledger-trade_pages', setup: week,
        steps: [P.bitvavo_ledger(night, balance: [P.bv_balance(%w[BTC 1])], trades: [P.ok(trade_page), P.ok(trades)])] },
      { name: 'bitvavo-ledger-balance_fails', setup: week,
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(deposits)], balance: [{ 'status' => 403, 'body' => { 'errorCode' => 311, 'error' => 'This key does not allow access to view endpoints.' } }])] },
      { name: 'bitvavo-ledger-trades_fail', setup: week,
        steps: [P.bitvavo_ledger(night, deposits: [P.ok(deposits)], balance: [P.bv_balance(%w[BTC 1])],
                                 trades: [{ 'status' => 500, 'body' => { 'errorCode' => 101, 'error' => 'Unknown server error.' } }])] },
      { name: 'bitvavo-ledger-network', setup: week,
        steps: [P.bitvavo_ledger(night, withdrawals: [{ 'network' => 'bitvavo_pre_send', 'message' => 'Connection refused - connect(2) for "api.bitvavo.com" port 443' }])] },
      { name: 'bitvavo-ledger-nothing', setup: week, steps: [P.bitvavo_ledger(night)] }
    ]
  end

  # Exchanges::Bitvavo#get_balances with AccountBalance::Sync (plan VB slice A3): one row per symbol, `available` and `inOrder`.
  def bitvavo_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25)
    rates = P.ok('data' => { 'btc' => { 'value' => 1 }, 'usd' => { 'value' => 60_000.5 }, 'eur' => { 'value' => 55_000.25 } })
    install = ->(ctx) { P.bitvavo(ctx) }
    seeded = lambda do |ctx|
      P.bitvavo(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2', exchange: ctx[:bitvavo])
    end
    err = ->(status, code, error) { { 'status' => status, 'body' => { 'errorCode' => code, 'error' => error } } }
    [
      { name: 'bitvavo-balances-first', setup: install,
        steps: [P.bitvavo_balances(night, balance: P.bv_balance(%w[BTC 0.5 0.1], %w[ETH 2], %w[EUR 100.5], %w[ADA 3], %w[DOGE 0]), rates:, prices: coins)] },
      { name: 'bitvavo-balances-invalid_key', setup: seeded, steps: [P.bitvavo_balances(night, balance: err.(403, 305, 'No active API key found.'))] },
      { name: 'bitvavo-balances-bad_signature', setup: seeded, steps: [P.bitvavo_balances(night, balance: err.(403, 309, 'The signature is invalid.'))] },
      { name: 'bitvavo-balances-no_view', setup: seeded, steps: [P.bitvavo_balances(night, balance: err.(403, 311, 'This key does not allow access to view endpoints.'))] },
      { name: 'bitvavo-balances-throttled', setup: seeded, steps: [P.bitvavo_balances(night, balance: err.(429, 105, 'Your account has been temporarily banned.'))] },
      { name: 'bitvavo-balances-not_json', setup: seeded, steps: [P.bitvavo_balances(night, balance: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'bitvavo-balances-network', setup: seeded,
        steps: [P.bitvavo_balances(night, balance: { 'network' => 'bitvavo_pre_send', 'message' => 'Connection refused - connect(2) for "api.bitvavo.com" port 443' })] },
      { name: 'bitvavo-balances-no_symbol', setup: seeded,
        steps: [P.bitvavo_balances(night, balance: P.ok([{ 'available' => '1', 'inOrder' => '0' }, { 'symbol' => 'BTC', 'available' => '0.3', 'inOrder' => '0' }]), prices: coins)] },
      { name: 'bitvavo-balances-no_in_order', setup: seeded, steps: [P.bitvavo_balances(night, balance: P.ok([{ 'symbol' => 'BTC', 'available' => '1' }]))] },
      { name: 'bitvavo-balances-nothing', setup: seeded, steps: [P.bitvavo_balances(night, balance: P.bv_balance)] }
    ]
  end

  # Exchanges::Gemini#get_ledger with AccountTransactionSync (plan VB B3, audit M4-M6): every trade from /v1/mytrades, then
  # every Deposit and Withdrawal from /v2/transfers, each walked forward from the newest timestampms of a full page (its
  # millisecond read again, its ids dropping the repeats) until a short page; a full page with nothing new, an answer that
  # is not a list or any failed read fails the run. A trade's pair is the listed ticker's, else the symbol split at the
  # longest known quote.
  def gemini_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    t = ->(*args) { Time.utc(*args) }
    since = ->(watermark) { ->(ctx) { P.gemini(ctx); ctx[:key].update_columns(last_synced_at: watermark) } }
    week = since.(t.(2026, 9, 15))
    trades = [P.gm_trade(9001, 'btcusd', 'Buy', '0.001', '60500', t.(2026, 9, 16, 10)),
              P.gm_trade(9002, 'ethbtc', 'Sell', '0.5', '0.04', t.(2026, 9, 17, 11), fee: '0.00002', fee_currency: 'BTC'),
              P.gm_trade(9003, 'dogeusd', 'Buy', '100', '0.25', 1_789_700_000_123, fee: nil, fee_currency: nil)] # unlisted: split at USD
    transfers = [P.gm_transfer(5001, 'Deposit', 'USD', '1000', t.(2026, 9, 15, 12)),
                 P.gm_transfer(5002, 'Withdrawal', 'btc', '0.002', t.(2026, 9, 18), fee: '0.0001', fee_currency: 'btc'),
                 P.gm_transfer(5003, 'Reward', 'ETH', '0.01', t.(2026, 9, 18, 1)),
                 P.gm_transfer(5004, 'Withdrawal', 'ETH', '0.1', t.(2026, 9, 18, 2), fee: '0')]
    # A full page of 500 trades ending at one millisecond, then the next page from it: the boundary trades come back with two
    # new ones, and a short page ends the source.
    base_ms = P.ms(t.(2026, 9, 16))
    full = (1..500).map { |i| P.gm_trade(10_000 + i, 'btcusd', 'Buy', '0.0001', '60000', base_ms + [i, 498].min * 1000) }
    second = full.select { |r| r['timestampms'] == full.last['timestampms'] } + [P.gm_trade(10_501, 'btcusd', 'Sell', '0.0001', '60100', base_ms + 600_000),
                                                                                P.gm_trade(10_502, 'ethusd', 'Buy', '0.01', '2500', base_ms + 700_000)]
    stuck = (1..500).map { |i| P.gm_trade(20_000 + i, 'btcusd', 'Buy', '0.0001', '60000', base_ms) }
    transfers_full = (1..50).map { |i| P.gm_transfer(6000 + i, 'Deposit', 'USD', '10', base_ms + i * 1000) }
    [
      { name: 'gemini-ledger-week', setup: week, steps: [P.gemini_ledger(night, trades: [P.ok(trades)], transfers: [P.ok(transfers)])] },
      { name: 'gemini-ledger-full', setup: ->(ctx) { P.gemini(ctx) }, steps: [P.gemini_ledger(night, trades: [P.ok(trades.first(1))], transfers: [P.ok(transfers.first(1))])] },
      { name: 'gemini-ledger-two_nights', setup: week,
        steps: [P.gemini_ledger(night, trades: [P.ok(trades)]), P.gemini_ledger('2026-09-21T02:00:00.250000Z', trades: [P.ok(trades)], transfers: [P.ok(transfers)])] },
      { name: 'gemini-ledger-pages', setup: week, steps: [P.gemini_ledger(night, trades: [P.ok(full), P.ok(second)], transfers: [P.ok(transfers_full), P.ok(transfers.first(2))])] },
      { name: 'gemini-ledger-stuck', setup: week, steps: [P.gemini_ledger(night, trades: [P.ok(stuck)])] },
      { name: 'gemini-ledger-not_a_list', setup: week, steps: [P.gemini_ledger(night, trades: [P.ok('result' => 'ok')])] },
      { name: 'gemini-ledger-invalid_key', setup: week,
        steps: [P.gemini_ledger(night, trades: [{ 'status' => 400, 'body' => { 'result' => 'error', 'reason' => 'InvalidSignature', 'message' => 'InvalidSignature' } }])] },
      { name: 'gemini-ledger-missing_role', setup: week,
        steps: [P.gemini_ledger(night, trades: [P.ok(trades)], transfers: [{ 'status' => 403, 'body' => { 'result' => 'error', 'reason' => 'MissingRole', 'message' => 'API key lacks the required role' } }])] },
      { name: 'gemini-ledger-failed_page', setup: week,
        steps: [P.gemini_ledger(night, trades: [P.ok(full), { 'status' => 500, 'body' => { 'result' => 'error', 'reason' => 'System', 'message' => 'Internal error' } }])] },
      { name: 'gemini-ledger-network', setup: week,
        steps: [P.gemini_ledger(night, transfers: [{ 'network' => 'gemini_pre_send', 'message' => 'Connection refused - connect(2) for "api.gemini.com" port 443' }])] },
      { name: 'gemini-ledger-nothing', setup: week, steps: [P.gemini_ledger(night)] }
    ]
  end

  # Exchanges::Gemini#get_balances with AccountBalance::Sync: each row's upcased currency by the venue's tickers, `available`
  # free and `amount - available` locked; a null answer is "Failed to get Gemini balances".
  def gemini_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25)
    install = ->(ctx) { P.gemini(ctx) }
    seeded = lambda do |ctx|
      P.gemini(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2', exchange: ctx[:gemini])
      P.balance(ctx, 'ETH', free: '1', usd_price: '2400', usd_value: '2400', exchange: ctx[:gemini])
    end
    err = ->(status, reason, message) { { 'status' => status, 'body' => { 'result' => 'error', 'reason' => reason, 'message' => message } } }
    [
      # A currency the venue lists no ticker for (GUSD) is not a balance; lowercase currencies are upcased.
      { name: 'gemini-balances-first', setup: install,
        steps: [P.gemini_balances(night, balances: P.gemini_rows(%w[BTC 0.5 0.6], %w[eth 2], %w[USD 100.5], %w[GUSD 3], %w[DOGE 0]), prices: coins)] },
      { name: 'gemini-balances-stale_and_gone', setup: seeded, steps: [P.gemini_balances(night, balances: P.gemini_rows(%w[BTC 0.45]), prices: P::SERVER_ERROR)] },
      { name: 'gemini-balances-invalid_key', setup: seeded, steps: [P.gemini_balances(night, balances: err.(400, 'InvalidApiKey', 'Invalid API key'))] },
      { name: 'gemini-balances-missing_role', setup: seeded, steps: [P.gemini_balances(night, balances: err.(403, 'MissingRole', 'API key lacks the required role'))] },
      { name: 'gemini-balances-throttled', setup: seeded, steps: [P.gemini_balances(night, balances: err.(429, 'RateLimit', 'Requests were made too frequently'))] },
      { name: 'gemini-balances-null', setup: seeded, steps: [P.gemini_balances(night, balances: P.ok(nil))] },
      { name: 'gemini-balances-not_json', setup: seeded, steps: [P.gemini_balances(night, balances: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'gemini-balances-network', setup: seeded,
        steps: [P.gemini_balances(night, balances: { 'network' => 'gemini_pre_send', 'message' => 'Connection refused - connect(2) for "api.gemini.com" port 443' })] },
      { name: 'gemini-balances-no_currency', setup: seeded, steps: [P.gemini_balances(night, balances: P.ok([{ 'available' => '1', 'amount' => '1' }]))] },
      { name: 'gemini-balances-no_amount', setup: seeded, steps: [P.gemini_balances(night, balances: P.ok([{ 'currency' => 'BTC', 'available' => '1' }]))] },
      { name: 'gemini-balances-nothing', setup: seeded, steps: [P.gemini_balances(night, balances: P.gemini_rows)] }
    ]
  end

  # Exchanges::Bitget#get_ledger with AccountTransactionSync (plan VB slice A4, AUDIT-bitget G6/G7): fills in 89-day windows from
  # the later of the start and 89 days ago, paged by idLessThan = tradeId; then, window by window from the start (2018-04-01
  # on a first sync), the deposit and the withdrawal records paged by orderId; 40014/40025 on a transfer read is an empty
  # read; any other error envelope or failure fails the run.
  def bitget_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    t = ->(*args) { Time.utc(*args) }
    since = ->(watermark) { ->(ctx) { P.bitget(ctx); ctx[:key].update_columns(last_synced_at: watermark) } }
    week = since.(t.(2026, 9, 15))
    fills = [P.bg_fill('9003', 'BTCUSDT', 'sell', '0.001', '64100', t.(2026, 9, 19, 10)),
             P.bg_fill('9002', 'ETHBTC', 'buy', '0.5', '0.031', t.(2026, 9, 18, 9), fee: '-0.0005', fee_coin: 'ETH'),
             P.bg_fill('9001', 'NEWUSDT', 'Buy', '10', '1.5', t.(2026, 9, 17), detail: false).merge('fee' => '-0.015')]
    deposits = [P.bg_transfer('d3', 'USDT', '1000', t.(2026, 9, 18, 12)), P.bg_transfer('d2', 'BTC', '0.01', t.(2026, 9, 17), status: 'pending'),
                P.bg_transfer('d1', 'ETH', '2', t.(2026, 9, 16), trade: '1234567')]
    withdrawals = [P.bg_transfer('w2', 'USDT', '50', t.(2026, 9, 19), fee: '-1.00000000', type: 'withdraw'),
                   P.bg_transfer('w1', 'BTC', '0.002', t.(2026, 9, 18), status: 'fail', fee: '-0.0001', type: 'withdraw')]
    # A full page (Bitget's 100), then the page before its last id; a page of 100 transfers likewise.
    full = Array.new(100) { |i| P.bg_fill((5000 - i).to_s, 'BTCUSDT', 'buy', '0.0001', '64000', t.(2026, 9, 19).to_i * 1000 - (i * 1000)) }
    transfers = Array.new(100) { |i| P.bg_transfer("p#{500 - i}", 'USDT', format('1.%03d', i), t.(2026, 9, 19).to_i * 1000 - (i * 60_000)) }
    bare = P.bg_fill('x', 'BTCUSDT', 'buy', '0.002', '64000', t.(2026, 9, 18, 8)).except('tradeId')
    scope = P.bg_err(400, '40014', 'Incorrect permissions')
    [
      { name: 'bitget-ledger-week', setup: week,
        steps: [P.bitget_ledger(night, fills: [P.bg_page(fills)], deposits: [P.bg_page(deposits)], withdrawals: [P.bg_page(withdrawals)])] },
      # A first sync: fills from 89 days ago, transfers window by window from 2018-04-01; the first window holds them.
      { name: 'bitget-ledger-full', setup: ->(ctx) { P.bitget(ctx) },
        steps: [P.bitget_ledger(night, fills: [P.bg_page(fills)], deposits: [P.bg_page(deposits), P.bg_page([])], withdrawals: [P.bg_page(withdrawals), P.bg_page([])])] },
      # Neighbouring windows share their edge: a record read in both is one row.
      { name: 'bitget-ledger-window_edge', setup: ->(ctx) { P.bitget(ctx) },
        steps: [P.bitget_ledger(night, deposits: [P.bg_page(deposits.first(1)), P.bg_page(deposits.first(1)), P.bg_page([])])] },
      { name: 'bitget-ledger-two_nights', setup: week,
        steps: [P.bitget_ledger(night, fills: [P.bg_page(fills)], deposits: [P.bg_page(deposits)]),
                P.bitget_ledger('2026-09-21T02:00:00.250000Z', fills: [P.bg_page(fills)], deposits: [P.bg_page(deposits)], withdrawals: [P.bg_page(withdrawals)])] },
      { name: 'bitget-ledger-fill_pages', setup: week, steps: [P.bitget_ledger(night, fills: [P.bg_page(full), P.bg_page(fills)])] },
      { name: 'bitget-ledger-transfer_pages', setup: week, steps: [P.bitget_ledger(night, deposits: [P.bg_page(transfers), P.bg_page(deposits)])] },
      # Two identical fills without a tradeId are one row (#get_ledger's uniq); another one is kept.
      { name: 'bitget-ledger-bare_fills', setup: week,
        steps: [P.bitget_ledger(night, fills: [P.bg_page([bare, bare, bare.merge('size' => '0.003')])])] },
      # A key without the wallet scope: its transfer reads are empty, its fills stored.
      { name: 'bitget-ledger-no_wallet_scope', setup: week,
        steps: [P.bitget_ledger(night, fills: [P.bg_page(fills)], deposits: [scope], withdrawals: [P.bg_err(400, '40025', 'The user does not have this permission')])] },
      { name: 'bitget-ledger-fills_refused', setup: week, steps: [P.bitget_ledger(night, fills: [scope], deposits: [P.bg_page(deposits)])] },
      { name: 'bitget-ledger-envelope_error', setup: week,
        steps: [P.bitget_ledger(night, fills: [P.bg_page(fills)], deposits: [P.ok('code' => '40015', 'msg' => 'System is abnormal', 'data' => [])])] },
      { name: 'bitget-ledger-invalid_key', setup: week, steps: [P.bitget_ledger(night, fills: [P.bg_err(400, '40037', 'Apikey does not exist')])] },
      { name: 'bitget-ledger-network', setup: week,
        steps: [P.bitget_ledger(night, fills: [P.bg_page(fills)], withdrawals: [{ 'network' => 'bitget_pre_send', 'message' => 'Connection refused - connect(2) for "api.bitget.com" port 443' }])] },
      { name: 'bitget-ledger-nothing', setup: week, steps: [P.bitget_ledger(night)] }
    ]
  end

  # Exchanges::Bitget#get_balances with AccountBalance::Sync (plan VB slice A4): the success code with `data`, one row per coin,
  # `available` and `frozen`.
  def bitget_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25)
    install = ->(ctx) { P.bitget(ctx) }
    seeded = lambda do |ctx|
      P.bitget(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2', exchange: ctx[:bitget])
      P.balance(ctx, 'ETH', free: '1', usd_price: '2400', usd_value: '2400', exchange: ctx[:bitget])
    end
    [
      # A coin the venue lists no ticker for (BGB) is not a balance; neither is a lowercase spelling (AUDIT-bitget S4).
      { name: 'bitget-balances-first', setup: install,
        steps: [P.bitget_balances(night, assets: P.bg_assets(%w[BTC 0.5 0.1], %w[ETH 2], %w[USDT 100.5], %w[BGB 3], %w[eth 7], %w[DOGE 0]), prices: coins)] },
      { name: 'bitget-balances-stale_and_gone', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_assets(%w[BTC 0.45]), prices: P::SERVER_ERROR)] },
      { name: 'bitget-balances-invalid_key', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_err(400, '40037', 'Apikey does not exist'))] },
      { name: 'bitget-balances-passphrase', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_err(400, '40036', 'passphrase is error'))] },
      { name: 'bitget-balances-no_permission', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_err(400, '40014', 'Incorrect permissions'))] },
      { name: 'bitget-balances-envelope_error', setup: seeded,
        steps: [P.bitget_balances(night, assets: P.ok('code' => '40037', 'msg' => 'Apikey does not exist', 'data' => []))] },
      { name: 'bitget-balances-throttled', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_err(429, '429', 'Too Many Requests'))] },
      { name: 'bitget-balances-not_json', setup: seeded, steps: [P.bitget_balances(night, assets: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'bitget-balances-network', setup: seeded,
        steps: [P.bitget_balances(night, assets: { 'network' => 'bitget_pre_send', 'message' => 'Connection refused - connect(2) for "api.bitget.com" port 443' })] },
      { name: 'bitget-balances-no_coin', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_page([{ 'available' => '1', 'frozen' => '0' }]))] },
      { name: 'bitget-balances-no_frozen', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_page([{ 'coin' => 'BTC', 'available' => '1' }]))] },
      { name: 'bitget-balances-nothing', setup: seeded, steps: [P.bitget_balances(night, assets: P.bg_assets)] }
    ]
  end

  # Exchanges::Mexc#get_ledger with AccountTransactionSync (plan VB slice A5, AUDIT-mexc X8-X12): deposits and withdrawals in
  # 7-day ranges from the start (89 days back at most), a full page halved, 700007 ending a history; then the pairs the
  # transfers, the account and the stored rows name; then their trades from the start (a month back at most), halved at 100.
  def mexc_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    t = ->(*args) { Time.utc(*args) }
    since = ->(watermark) { ->(ctx) { P.mexc(ctx); ctx[:key].update_columns(last_synced_at: watermark) } }
    week = since.(t.(2026, 9, 15))
    deposits = [P.mx_deposit('USDT', '1000', t.(2026, 9, 18, 12)), P.mx_deposit('BTC', '0.01', t.(2026, 9, 17), status: 12),
                P.mx_deposit('ETH', '2', t.(2026, 9, 16), status: 1), P.mx_deposit('USDT', '50', t.(2026, 9, 16, 1), status: 6)]
    withdrawals = [P.mx_withdrawal('w1', 'BTC', '0.002', t.(2026, 9, 19)), P.mx_withdrawal('w2', 'ETH', '0.1', t.(2026, 9, 18), status: 6)]
    trades = [P.mx_trade(3, 'BTCUSDT', false, '0.001', '64100', t.(2026, 9, 19, 10)),
              P.mx_trade(2, 'BTCUSDT', true, '0.002', '64000', t.(2026, 9, 18, 9), commission: '-0.01'),
              P.mx_trade(2, 'BTCUSDT', true, '0.002', '64000', t.(2026, 9, 18, 9), commission: '-0.01')] # the same trade twice: once
    info = P.mx_info(%w[BTC USDT], %w[ETH USDT], %w[ETH BTC], %w[ADA USDT], %w[XRP USDT])
    full = ->(n, coin, from) { Array.new(n) { |i| P.mx_deposit(coin, format('1.%04d', i), from + (i * 60)) } }
    gone = { 'status' => 400, 'body' => { 'code' => 700_007, 'msg' => 'No permission to access the endpoint.' } }
    [
      { name: 'mexc-ledger-week', setup: week,
        steps: [P.mexc_ledger(night, deposits: [P.ok(deposits)], withdrawals: [P.ok(withdrawals)], account: [P.mx_account(%w[BTC 0.5], %w[ADA 0 0], %w[XRP 0 3])],
                              info: [info], trades: [P.ok(trades), P.ok([P.mx_trade(9, 'XRPUSDT', true, '10', '0.5', t.(2026, 9, 17), asset: 'XRP', commission: '0.01')])])] },
      # No watermark: thirteen 7-day ranges of each history back to 89 days, and the trades of a month.
      { name: 'mexc-ledger-full', setup: ->(ctx) { P.mexc(ctx) },
        steps: [P.mexc_ledger(night, deposits: [P.ok(deposits.first(2)), P.ok([])], info: [info], trades: [P.ok(trades)])] },
      # A watermark 100 days old: the start is floored to 80 days ago (#ledger_window).
      { name: 'mexc-ledger-away_100_days', setup: since.(t.(2026, 6, 12)),
        steps: [P.mexc_ledger(night, deposits: [P.ok([P.mx_deposit('BTC', '0.3', t.(2026, 7, 2))]), P.ok([])], info: [info], trades: [P.ok([])])] },
      { name: 'mexc-ledger-two_nights', setup: week,
        steps: [P.mexc_ledger(night, deposits: [P.ok(deposits)], info: [info], trades: [P.ok(trades)]),
                P.mexc_ledger('2026-09-21T02:00:00.250000Z', deposits: [P.ok(deposits)], withdrawals: [P.ok(withdrawals)], info: [info],
                              trades: [P.ok(trades), P.ok([])])] },
      # A full page of deposits is read again in halves, older first; the halves' rows are kept, the full page's not.
      { name: 'mexc-ledger-halving', setup: week,
        steps: [P.mexc_ledger(night, deposits: [P.ok(full.(1000, 'USDT', t.(2026, 9, 15))), P.ok(full.(3, 'ETH', t.(2026, 9, 15))),
                                                P.ok(full.(2, 'BTC', t.(2026, 9, 18)))], info: [info])] },
      # 700007 in a half ends the deposits: the range's other half is not kept, the withdrawals are still read.
      { name: 'mexc-ledger-no_permission_mid', setup: week,
        steps: [P.mexc_ledger(night, deposits: [P.ok(full.(1000, 'USDT', t.(2026, 9, 15))), P.ok(full.(3, 'ETH', t.(2026, 9, 15))), gone],
                              withdrawals: [P.ok(withdrawals)], info: [info])] },
      { name: 'mexc-ledger-no_permission', setup: week,
        steps: [P.mexc_ledger(night, deposits: [gone], withdrawals: [gone], account: [P.mx_account(%w[BTC 1])], info: [info], trades: [P.ok(trades)])] },
      # A full page of trades (100) is halved.
      { name: 'mexc-ledger-trade_halving', setup: week,
        steps: [P.mexc_ledger(night, account: [P.mx_account(%w[BTC 1])], info: [info],
                              trades: [P.ok(Array.new(100) { |i| P.mx_trade(100 + i, 'BTCUSDT', true, '0.0001', '64000', t.(2026, 9, 18)) }),
                                       P.ok(trades), P.ok([P.mx_trade(500, 'BTCUSDT', false, '0.0002', '64500', t.(2026, 9, 19, 12))])])] },
      # A pair MEXC no longer lists is skipped; a sold-out pair the stored rows name is read (X12).
      { name: 'mexc-ledger-symbol_gone',
        setup: lambda do |ctx|
          week.(ctx)
          AccountTransaction.create!(user: ctx[:user], api_key: ctx[:key], exchange: ctx[:mexc], entry_type: :sell, base_currency: 'ETH', base_amount: 1,
                                     quote_currency: 'USDT', quote_amount: 2500, tx_id: 'old-1', transacted_at: t.(2026, 8, 1), raw_data: {})
        end,
        steps: [P.mexc_ledger(night, account: [P.mx_account(%w[BTC 1])], info: [info],
                              trades: [{ 'status' => 400, 'body' => { 'code' => -1121, 'msg' => 'Invalid symbol.' } },
                                       P.ok([P.mx_trade(7, 'ETHUSDT', false, '0.5', '2600', t.(2026, 9, 17))])])] },
      { name: 'mexc-ledger-trades_fail', setup: week,
        steps: [P.mexc_ledger(night, deposits: [P.ok(deposits)], account: [P.mx_account(%w[BTC 1])], info: [info],
                              trades: [{ 'status' => 500, 'body' => { 'code' => 503, 'msg' => 'service not available' } }])] },
      { name: 'mexc-ledger-account_fails', setup: week,
        steps: [P.mexc_ledger(night, deposits: [P.ok(deposits)], account: [{ 'status' => 400, 'body' => { 'code' => 10_072, 'msg' => 'Api key info invalid' } }])] },
      { name: 'mexc-ledger-not_a_list', setup: week,
        steps: [P.mexc_ledger(night, withdrawals: [P.ok('code' => 1, 'msg' => 'odd "answer"', 'data' => nil)])] },
      { name: 'mexc-ledger-network', setup: week,
        steps: [P.mexc_ledger(night, withdrawals: [{ 'network' => 'pre_send:api.mexc.com', 'message' => 'Connection refused - connect(2) for "api.mexc.com" port 443' }])] },
      { name: 'mexc-ledger-nothing', setup: week, steps: [P.mexc_ledger(night)] }
    ]
  end

  # Exchanges::Mexc#get_balances with AccountBalance::Sync (plan VB slice A5): Binance's account shape, `free` and `locked`.
  def mexc_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25, 'tether' => 1.0001)
    install = ->(ctx) { P.mexc(ctx) }
    seeded = lambda do |ctx|
      P.mexc(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2', exchange: ctx[:mexc])
    end
    err = ->(status, code, msg) { { 'status' => status, 'body' => { 'code' => code, 'msg' => msg } } }
    [
      { name: 'mexc-balances-first', setup: install,
        steps: [P.mexc_balances(night, account: P.mx_account(%w[BTC 0.5 0.1], %w[ETH 2], %w[USDT 100.5], %w[MX 3], %w[DOGE 0]), prices: coins)] },
      { name: 'mexc-balances-invalid_key', setup: seeded, steps: [P.mexc_balances(night, account: err.(400, 10_072, 'Api key info invalid'))] },
      { name: 'mexc-balances-bad_signature', setup: seeded, steps: [P.mexc_balances(night, account: err.(400, 700_002, 'Signature for this request is not valid.'))] },
      { name: 'mexc-balances-no_permission', setup: seeded, steps: [P.mexc_balances(night, account: err.(403, 700_007, 'No permission to access the endpoint.'))] },
      { name: 'mexc-balances-ip', setup: seeded, steps: [P.mexc_balances(night, account: err.(403, 700_006, 'IP non white list'))] },
      { name: 'mexc-balances-throttled', setup: seeded, steps: [P.mexc_balances(night, account: err.(429, 429, 'Too Many Requests'))] },
      { name: 'mexc-balances-not_json', setup: seeded, steps: [P.mexc_balances(night, account: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'mexc-balances-bad_gateway', setup: seeded, steps: [P.mexc_balances(night, account: { 'status' => 502, 'body' => '<html>502 Bad Gateway</html>' })] },
      { name: 'mexc-balances-network', setup: seeded,
        steps: [P.mexc_balances(night, account: { 'network' => 'pre_send:api.mexc.com', 'message' => 'Connection refused - connect(2) for "api.mexc.com" port 443' })] },
      { name: 'mexc-balances-no_locked', setup: seeded, steps: [P.mexc_balances(night, account: P.ok('balances' => [{ 'asset' => 'BTC', 'free' => '1' }]))] },
      { name: 'mexc-balances-no_balances', setup: seeded, steps: [P.mexc_balances(night, account: P.ok('canTrade' => true))] },
      { name: 'mexc-balances-nothing', setup: seeded, steps: [P.mexc_balances(night, account: P.mx_account)] }
    ]
  end

  # Exchanges::Hyperliquid#get_ledger with AccountTransactionSync (plan VB slice B1, audit H5-H7): every fill from
  # userFillsByTime, then every update from userNonFundingLedgerUpdates, each from startTime (the watermark less 25 h, or 0),
  # the next page from the newest `time` of a full page (2000 fills, 500 updates), identical rows counted by the page that holds
  # them most; a full page with nothing new or any failed read fails the run. Spot fills only ("@N" or a pair name), base and
  # quote from the tickers (USDC for an unlisted pair), a positive fee in feeToken; spot transfers by their delta's type.
  def hyperliquid_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    t = ->(*args) { Time.utc(*args) }
    me = P::HL_KEY['key'].downcase
    since = ->(watermark) { ->(ctx) { P.hyperliquid(ctx); ctx[:key].update_columns(last_synced_at: watermark) } }
    week = since.(t.(2026, 9, 15))
    fills = [P.hl_fill(9001, '@142', 'B', '0.001', '60500', t.(2026, 9, 16, 10)),
             P.hl_fill(9002, '@151', 'A', '0.5', '2400', t.(2026, 9, 17, 11), fee: '0.6', fee_token: 'USDT'),
             P.hl_fill(9003, 'ETH/BTC', 'B', '0.2', '0.04', t.(2026, 9, 17, 12), fee: '0.00001', fee_token: 'ETH'),
             P.hl_fill(9004, '@999', 'B', '100', '0.25', t.(2026, 9, 18), fee: '0', fee_token: nil), # unlisted: its coin, USDC
             P.hl_fill(9005, '@142', 'A', '0.002', '61000', t.(2026, 9, 18, 1), fee: '-0.01'), # a maker rebate is no fee
             P.hl_fill(9006, 'BTC', 'B', '0.01', '61000', t.(2026, 9, 18, 2), fee: '0.3', fee_token: 'USDC')] # a perp fill
    staking = P.hl_update('0x' + '0' * 64, t.(2026, 9, 18, 6), 'type' => 'cStakingTransfer', 'token' => 'HYPE', 'amount' => '5.0', 'isDeposit' => true)
    updates = [P.hl_update('0xa1', t.(2026, 9, 16), 'type' => 'spotTransfer', 'token' => 'USDT', 'amount' => '100.0', 'usdcValue' => '100.0', 'user' => me.upcase,
                           'destination' => '0xother', 'fee' => '1.0', 'nativeTokenFee' => '0.0', 'nonce' => nil, 'feeToken' => 'USDT'),
               P.hl_update('0xa2', t.(2026, 9, 16, 1), 'type' => 'spotTransfer', 'token' => 'BTC', 'amount' => '0.01', 'usdcValue' => '600', 'user' => '0xother',
                           'destination' => me, 'fee' => '0.0', 'nativeTokenFee' => '0.0', 'nonce' => nil),
               P.hl_update('0xa3', t.(2026, 9, 16, 2), 'type' => 'send', 'token' => 'ETH', 'amount' => '0.1', 'usdcValue' => '240', 'user' => me,
                           'destination' => '0xother', 'sourceDex' => 'spot', 'destinationDex' => 'spot', 'fee' => '0.0'),
               P.hl_update('0xa4', t.(2026, 9, 16, 3), 'type' => 'send', 'token' => 'USDT', 'amount' => '5', 'user' => me, 'destination' => me,
                           'sourceDex' => '', 'destinationDex' => 'spot', 'fee' => '0.0'),
               P.hl_update('0xa5', t.(2026, 9, 16, 4), 'type' => 'send', 'token' => 'USDT', 'amount' => '5', 'user' => me, 'destination' => me,
                           'sourceDex' => 'spot', 'destinationDex' => 'spot', 'fee' => '0.0'), # spot to spot of itself: no row
               P.hl_update('0xa6', t.(2026, 9, 17), 'type' => 'accountClassTransfer', 'usdc' => '250.0', 'toPerp' => false),
               P.hl_update('0xa7', t.(2026, 9, 17, 1), 'type' => 'accountClassTransfer', 'usdc' => '20.0', 'toPerp' => true),
               staking, staking, # two equal zero-hash rows in one page: both kept
               P.hl_update('0xa8', t.(2026, 9, 18, 7), 'type' => 'cStakingTransfer', 'token' => 'HYPE', 'amount' => '1.0', 'isDeposit' => false),
               P.hl_update('0xa9', t.(2026, 9, 18, 8), 'type' => 'spotGenesis', 'token' => 'PURR', 'amount' => '1000'),
               P.hl_update('0xaa', t.(2026, 9, 18, 9), 'type' => 'deposit', 'usdc' => '500.0'), # into perp: no row
               P.hl_update('0xab', t.(2026, 9, 18, 10), 'type' => 'withdraw', 'usdc' => '50.0', 'nonce' => 1, 'fee' => '1.0')]
    # A full page of 2000 fills ending at one millisecond, then the next page from it: the boundary fills come back with two new
    # ones; a short page ends the read.
    base_ms = P.ms(t.(2026, 9, 16))
    full = (1..2000).map { |i| P.hl_fill(10_000 + i, '@142', 'B', '0.0001', '60000', base_ms + [i, 1998].min * 1000) }
    second = full.select { |r| r['time'] == full.last['time'] } + [P.hl_fill(12_001, '@142', 'A', '0.0001', '60100', base_ms + 3_000_000),
                                                                   P.hl_fill(12_002, '@151', 'B', '0.01', '2500', base_ms + 4_000_000)]
    # A full page of 500 updates whose last millisecond holds the two equal staking rows; the next page serves them again (not
    # tripled) and one more.
    ups = (1..498).map { |i| P.hl_update(format('0xb%03d', i), base_ms + i * 1000, 'type' => 'accountClassTransfer', 'usdc' => '1.0', 'toPerp' => false) }
    boundary = P.hl_update('0x' + '0' * 64, base_ms + 600_000, 'type' => 'cStakingTransfer', 'token' => 'HYPE', 'amount' => '2.0', 'isDeposit' => false)
    ups_full = ups + [boundary, boundary]
    ups_next = [boundary, boundary, P.hl_update('0xc1', base_ms + 700_000, 'type' => 'spotGenesis', 'token' => 'PURR', 'amount' => '5')]
    err = ->(status, body) { { 'status' => status, 'body' => body } }
    [
      { name: 'hyperliquid-ledger-week', setup: week, steps: [P.hyperliquid_ledger(night, fills: [P.ok(fills)], updates: [P.ok(updates)])] },
      { name: 'hyperliquid-ledger-full', setup: ->(ctx) { P.hyperliquid(ctx) },
        steps: [P.hyperliquid_ledger(night, fills: [P.ok(fills.first(2))], updates: [P.ok(updates.first(2))])] },
      { name: 'hyperliquid-ledger-two_nights', setup: week,
        steps: [P.hyperliquid_ledger(night, fills: [P.ok(fills)], updates: [P.ok(updates)]),
                P.hyperliquid_ledger('2026-09-21T02:00:00.250000Z', fills: [P.ok(fills)], updates: [P.ok(updates)])] },
      { name: 'hyperliquid-ledger-pages', setup: week, steps: [P.hyperliquid_ledger(night, fills: [P.ok(full), P.ok(second)], updates: [P.ok(ups_full), P.ok(ups_next)])] },
      { name: 'hyperliquid-ledger-stuck', setup: week, steps: [P.hyperliquid_ledger(night, fills: [P.ok(full)])] },
      { name: 'hyperliquid-ledger-null', setup: week, steps: [P.hyperliquid_ledger(night, fills: [P.ok(nil)], updates: [P.ok(updates.first(1))])] },
      { name: 'hyperliquid-ledger-unprocessable', setup: week,
        steps: [P.hyperliquid_ledger(night, fills: [err.(422, 'Failed to deserialize the JSON body into the target type')])] },
      { name: 'hyperliquid-ledger-failed_page', setup: week, steps: [P.hyperliquid_ledger(night, fills: [P.ok(full), err.(500, 'Internal error')])] },
      { name: 'hyperliquid-ledger-updates_failed', setup: week, steps: [P.hyperliquid_ledger(night, fills: [P.ok(fills)], updates: [err.(429, 'Too Many Requests')])] },
      { name: 'hyperliquid-ledger-network', setup: week,
        steps: [P.hyperliquid_ledger(night, updates: [{ 'network' => 'hyperliquid_pre_send', 'message' => 'Connection refused - connect(2) for "api.hyperliquid.xyz" port 443' }])] },
      { name: 'hyperliquid-ledger-nothing', setup: week, steps: [P.hyperliquid_ledger(night)] }
    ]
  end

  # Exchanges::Hyperliquid#get_balances with AccountBalance::Sync (plan VB slice B1): spotClearinghouseState's `balances`, each
  # row's coin by the venue's tickers, free = total - hold, hold locked.
  def hyperliquid_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    coins = P.prices_body('bitcoin' => 60_000.5, 'ethereum' => 2500.25)
    install = ->(ctx) { P.hyperliquid(ctx) }
    seeded = lambda do |ctx|
      P.hyperliquid(ctx)
      P.balance(ctx, 'BTC', free: '0.4', usd_price: '59000.5', usd_value: '23600.2', exchange: ctx[:hyperliquid])
      P.balance(ctx, 'ETH', free: '1', usd_price: '2400', usd_value: '2400', exchange: ctx[:hyperliquid])
    end
    err = ->(status, body) { { 'status' => status, 'body' => body } }
    [
      # A coin the venue lists no ticker for (PURR) is not a balance.
      { name: 'hyperliquid-balances-first', setup: install,
        steps: [P.hyperliquid_balances(night, balances: P.hl_rows(%w[BTC 0.5 0.1], %w[ETH 2.0], %w[USDT 100.5 0.0], %w[PURR 3], %w[DOGE 0.0]), prices: coins)] },
      { name: 'hyperliquid-balances-stale_and_gone', setup: seeded, steps: [P.hyperliquid_balances(night, balances: P.hl_rows(%w[BTC 0.45]), prices: P::SERVER_ERROR)] },
      { name: 'hyperliquid-balances-no_balances', setup: seeded, steps: [P.hyperliquid_balances(night, balances: P.ok('balances' => nil))] },
      { name: 'hyperliquid-balances-no_total', setup: seeded, steps: [P.hyperliquid_balances(night, balances: P.ok('balances' => [{ 'coin' => 'BTC', 'hold' => '0.1' }]))] },
      { name: 'hyperliquid-balances-unprocessable', setup: seeded,
        steps: [P.hyperliquid_balances(night, balances: err.(422, 'Failed to deserialize the JSON body into the target type'))] },
      { name: 'hyperliquid-balances-invalid_key', setup: seeded,
        steps: [P.hyperliquid_balances(night, balances: err.(400, 'User or API Wallet 0x0000000000000000000000000000000000000001 does not exist.'))] },
      { name: 'hyperliquid-balances-throttled', setup: seeded, steps: [P.hyperliquid_balances(night, balances: err.(429, 'Too Many Requests'))] },
      { name: 'hyperliquid-balances-not_json', setup: seeded, steps: [P.hyperliquid_balances(night, balances: { 'status' => 200, 'body' => 'maintenance' })] },
      { name: 'hyperliquid-balances-network', setup: seeded,
        steps: [P.hyperliquid_balances(night, balances: { 'network' => 'hyperliquid_pre_send', 'message' => 'Connection refused - connect(2) for "api.hyperliquid.xyz" port 443' })] },
      { name: 'hyperliquid-balances-nothing', setup: seeded, steps: [P.hyperliquid_balances(night, balances: P.hl_rows)] }
    ]
  end

  def bybit_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    week = ->(ctx) { P.bybit(ctx); ctx[:key].update_columns(last_synced_at: Time.iso8601(night) - 3.days) }
    fill = { 'symbol' => 'BTCUSDT', 'side' => 'Buy', 'execQty' => '0.001', 'execValue' => '60', 'execFee' => '-0.06', 'feeCurrency' => 'USDT',
             'execId' => 'f1', 'execTime' => P.ms('2026-09-19T10:00:00Z').to_s }
    dep = { 'coin' => 'USDT', 'amount' => '100', 'status' => 3, 'txID' => 'd1', 'successAt' => fill['execTime'] }
    internal = dep.merge('status' => 2, 'txID' => 'i1', 'createdTime' => '1789812000')
    withdrawal = dep.merge('status' => 'success', 'txID' => 'w1', 'withdrawFee' => '1', 'updateTime' => fill['execTime'])
    rows = [
      { name: 'bybit-ledger-week', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_page([fill], key: 'list')],
        deposits: [P.by_page([dep, dep.merge('status' => 70012, 'txID' => 'rollback'), dep.merge('status' => 10012, 'txID' => 'ignored')])],
        internal: [P.by_page([internal])], withdrawals: [P.by_page([withdrawal, withdrawal.merge('status' => 'BlockchainConfirmed', 'txID' => 'ignored')])])] },
      { name: 'bybit-ledger-full', setup: ->(ctx) { P.bybit(ctx) }, steps: [P.bybit_ledger(night, fills: [P.by_page([fill], key: 'list'), P.by_page([], key: 'list')])] },
      { name: 'bybit-ledger-cursor', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_page([fill], key: 'list', cursor: 'next+/='), P.by_page([fill.merge('execId' => 'f2')], key: 'list')])] },
      { name: 'bybit-ledger-transfer_cursor', setup: week, steps: [P.bybit_ledger(night, deposits: [P.by_page([dep], cursor: 'd2'), P.by_page([dep.merge('txID' => 'd2')])],
          internal: [P.by_page([internal], cursor: 'i2'), P.by_page([internal.merge('txID' => 'i2', 'createdTime' => fill['execTime'])])],
          withdrawals: [P.by_page([withdrawal], cursor: 'w2'), P.by_page([withdrawal.merge('txID' => 'w2')])])] },
      { name: 'bybit-ledger-no_wallet_scope', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_page([fill], key: 'list')], deposits: [P.by_err(10005, 'Permission denied')], internal: [P.by_err(10005, 'Permission denied')], withdrawals: [P.by_err(10005, 'Permission denied')])] },
      { name: 'bybit-ledger-trades_refused', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_err(10005, 'Permission denied')])] },
      { name: 'bybit-ledger-failed_page', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_page([fill], key: 'list', cursor: 'next'), P.by_err(10016, 'Server error.')])] },
      # The final failure bounds Rails' repeated-cursor walk; Rust refuses at the repetition (D-BYBIT-STUCK).
      { name: 'bybit-ledger-stuck', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_page([fill], key: 'list', cursor: 'same'), P.by_page([fill], key: 'list', cursor: 'same'), P.by_err(10016, 'Server error.')])] },
      { name: 'bybit-ledger-throttled', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_err(10006, 'Too many visits'), P.by_page([fill], key: 'list')])] },
      { name: 'bybit-ledger-bare', setup: week, steps: [P.bybit_ledger(night, fills: [P.by_page([fill.except('execId'), fill.except('execId')], key: 'list')])] },
      { name: 'bybit-ledger-nothing', setup: week, steps: [P.bybit_ledger(night)] }
    ]
    rows << { name: 'bybit-ledger-two_nights', setup: week, steps: [P.bybit_ledger(night, deposits: [P.by_page([dep])]), P.bybit_ledger('2026-09-21T02:00:00.250000Z', deposits: [P.by_page([dep])])] }
    rows
  end
  def bybit_balance_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    setup = ->(ctx) { P.bybit(ctx) }
    cases = {
      'wallet' => [{ 'coin' => 'BTC', 'walletBalance' => '0.5', 'locked' => '0.1', 'spotBorrow' => '0.05', 'availableToWithdraw' => '99' }],
      'floor' => [{ 'coin' => 'BTC', 'walletBalance' => '0.1', 'locked' => '0.2', 'spotBorrow' => '0.05' }],
      'legacy' => [{ 'coin' => 'BTC', 'walletBalance' => '', 'availableToWithdraw' => '0.2', 'locked' => '0.1' }, { 'coin' => 'USDT', 'free' => '10' }],
      'unknown' => [{ 'coin' => 'UNLISTED', 'walletBalance' => '10' }, { 'walletBalance' => '99' }],
      'invalid_key' => P.by_err(10003, 'API key is invalid.'), 'permission' => P.by_err(10005, 'Permission denied, please check your API key permissions'),
      'empty' => [], 'duplicate' => [{ 'coin' => 'USDT', 'walletBalance' => '10' }, { 'coin' => 'USDT', 'walletBalance' => '20' }]
    }
    cases.map { |name, coins| { name: "bybit-balances-#{name}", setup: setup, steps: [P.bybit_balances(night, coins: coins, prices: P.prices_body('bitcoin' => 60000.5, 'ethereum' => 2500.25, 'tether' => 1.0))] } }
  end

  def bingx_ledger_scenarios
    night='2026-09-20T02:00:00.250000Z'; at=Time.utc(2026,9,18)
    week=->(ctx) {P.bingx(ctx);ctx[:key].update_columns(last_synced_at:Time.utc(2026,9,15))}
    dep={'coin'=>'USDTTRC20','network'=>'TRC20','amount'=>'12.5','txId'=>'dep1','status'=>1,'insertTime'=>P.ms(at)}
    wd={'id'=>'w1','txId'=>'batched','coin'=>'USDTTRC20','network'=>'TRC20','amount'=>'3','transactionFee'=>'1','status'=>6,'applyTime'=>'2026-09-18T04:00:00+02:00'}
    trade=P.mx_trade('t1','BTC-USDT',true,'0.001','64000',at,commission:'-0.01')
    fills=->(a) {P.bx_page('fills'=>a)};err=->(c,m) {P.ok('code'=>c,'msg'=>m)}
    full=Array.new(500) { |i| trade.merge('id'=>"t#{i}") }; transfers=Array.new(1000) { |i| dep.merge('txId'=>"d#{i}") }
    cases=[
      {name:'bingx-ledger-week',setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok([dep])],withdrawals:[P.ok([wd,wd.merge('id'=>'w2')])],trades:[fills.([trade])])]},
      {name:'bingx-ledger-full',setup:->(ctx) {P.bingx(ctx)},steps:[P.bingx_ledger(night,trades:[fills.([trade])])]},
      {name:'bingx-ledger-two_nights',setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok([dep])],trades:[fills.([trade])]),P.bingx_ledger('2026-09-21T02:00:00.250000Z',deposits:[P.ok([dep])],trades:[fills.([trade])])]},
      {name:'bingx-ledger-halving',setup:week,steps:[P.bingx_ledger(night,trades:[fills.(full),fills.([trade]),fills.([trade])])]},
      {name:'bingx-ledger-saturated',setup:week,steps:[P.bingx_ledger(night,trades:[fills.(full)])]},
      {name:'bingx-ledger-offset',setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok(transfers),P.ok([dep])])]},
      {name:'bingx-ledger-repeated_page',setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok(transfers)])]},
      {name:'bingx-ledger-no_permission',setup:week,steps:[P.bingx_ledger(night,deposits:[err.(100004,'API key was created without the permission')],withdrawals:[err.(100004,'API key was created without the permission')])]},
      {name:'bingx-ledger-moved_pair',setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok([dep.merge('coin'=>'ETH','network'=>'')])],account:[P.bx_account],info:[P.bx_info(%w[ETH USDT])],trades:[fills.([trade])])]},
      {name:'bingx-ledger-pending',setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok([dep.merge('status'=>0)])],withdrawals:[P.ok([wd.merge('status'=>0)])])]},
      {name:'bingx-ledger-nothing',setup:week,steps:[P.bingx_ledger(night)]}
    ]
    [100204,100490].each { |c|cases << {name:"bingx-ledger-no_trades_#{c}",setup:week,steps:[P.bingx_ledger(night,trades:[err.(c,'Requested data not found')])]} }
    %i[deposits withdrawals account info trades].each { |source|cases << {name:"bingx-ledger-failure_#{source}",setup:week,steps:[P.bingx_ledger(night,**{source=>[err.(100500,'Internal system error')]})]} }
    cases << {name:'bingx-ledger-sold_out',setup:lambda { |ctx|
      week.call(ctx)
      AccountTransaction.create!(user:ctx[:user],api_key:ctx[:key],exchange:ctx[:bingx],entry_type: :sell,base_currency:'ETH',base_amount:1,
        quote_currency:'USDT',quote_amount:2500,transacted_at:at-10.days,tx_id:'old-sale')
    },steps:[P.bingx_ledger(night,account:[P.bx_account],info:[P.bx_info(%w[ETH USDT])],trades:[fills.([trade])])]}
    cases << {name:'bingx-ledger-bot_order_pair',setup:lambda { |ctx|
      week.call(ctx)
      bot=P.holder(ctx,'ETH',at:at-10.days,venue:ctx[:bingx])
      bot.transactions.update_all(base:'ETH',quote:'USDT',quote_asset_id:ctx[:assets]['USDT'].id)
    },steps:[P.bingx_ledger(night,account:[P.bx_account],info:[P.bx_info(%w[ETH USDT])],trades:[fills.([trade])])]}
    # The account and listing answers Rails reads with dig and Array(): some raise, some read as nothing held or listed.
    {account:'balances',info:'symbols'}.each do |source,rows|
      {'null'=>nil,'array'=>[],'string'=>'bad','data_array'=>{'code'=>0,'data'=>[]},'data_string'=>{'code'=>0,'data'=>'bad'},
       'missing_data'=>{'code'=>0},'null_rows'=>{'code'=>0,'data'=>{rows=>nil}},'empty_rows'=>{'code'=>0,'data'=>{rows=>[]}},
       'object_rows'=>{'code'=>0,'data'=>{rows=>{'bad'=>1}}},'string_rows'=>{'code'=>0,'data'=>{rows=>'bad'}},
       'number_rows'=>{'code'=>0,'data'=>{rows=>1}},'false_rows'=>{'code'=>0,'data'=>{rows=>false}},'empty_row'=>{'code'=>0,'data'=>{rows=>[{}]}}}.each do |shape,body|
        cases << {name:"bingx-ledger-#{source}_#{shape}",setup:week,steps:[P.bingx_ledger(night,deposits:[P.ok([dep])],**{source=>[P.ok(body)]},trades:[fills.([trade])])]}
      end
    end
    cases
  end
  def bingx_balance_scenarios
    night='2026-09-20T02:00:00.250000Z';install=->(ctx) {P.bingx(ctx)}
    cases=[{name:'bingx-balances-held',setup:install,steps:[P.bingx_balances(night,account:P.bx_account(%w[BTC 0.5 0.1],%w[USDT 100]),prices:P.prices_body('bitcoin'=>60000))]},
      {name:'bingx-balances-empty',setup:install,steps:[P.bingx_balances(night,account:P.bx_account)]}]
    {100413=>'Incorrect apiKey',100004=>'API key was created without the permission',100410=>'rate limitation'}.each do |c,msg|
      cases << {name:"bingx-balances-error_#{c}",setup:install,steps:[P.bingx_balances(night,account:P.ok('code'=>c,'msg'=>msg))]}
    end
    cases
  end

  def bitrue_ledger_scenarios
    night = '2026-09-20T02:00:00.250000Z'
    at = Time.utc(2026, 9, 18)
    week = ->(ctx) { P.bitrue(ctx); ctx[:key].update_columns(last_synced_at: Time.utc(2026, 9, 15)) }
    trades = [P.br_trade(1, at), P.br_trade(2, at).merge('isBuyer' => false, 'commissionAsset' => nil, 'commissionAssert' => 'btc')]
    dep = P.br_transfer('d1', 'BTC', at)
    wd = P.br_transfer('w1', 'usdt_erc20', at, fee: '0.1')
    full = Array.new(1000) { |i| P.br_trade(i + 1000, at) }
    transfers = Array.new(1000) { |i| P.br_transfer("p#{i}", 'BTC', at) }
    err = ->(status, code, msg) { { 'status' => status, 'body' => { 'code' => code, 'msg' => msg } } }
    [
      { name: 'bitrue-ledger-week', setup: week, steps: [P.bitrue_ledger(night, trades: [P.ok(trades)], deposits: [P.br_page([dep, dep.except('id')])], withdrawals: [P.br_page([wd])])] },
      { name: 'bitrue-ledger-full', setup: ->(ctx) { P.bitrue(ctx) }, steps: [P.bitrue_ledger(night, trades: [P.ok(trades), P.ok([])])] },
      { name: 'bitrue-ledger-two_years', setup: ->(ctx) { P.bitrue(ctx); ctx[:key].update_columns(last_synced_at: Time.utc(2024, 9, 20)) },
        steps: [P.bitrue_ledger(night, trades: [P.ok(trades), P.ok(trades), P.ok([])])] },
      { name: 'bitrue-ledger-two_nights', setup: week,
        steps: [P.bitrue_ledger(night, trades: [P.ok(trades)], deposits: [P.br_page([dep])]), P.bitrue_ledger('2026-09-21T02:00:00.250000Z', trades: [P.ok(trades)], deposits: [P.br_page([dep])])] },
      { name: 'bitrue-ledger-halving', setup: week, steps: [P.bitrue_ledger(night, trades: [P.ok(full), P.ok(trades), P.ok(trades)])] },
      { name: 'bitrue-ledger-trade_saturated', setup: week, steps: [P.bitrue_ledger(night, trades: [P.ok(full)])] },
      { name: 'bitrue-ledger-offset', setup: week, steps: [P.bitrue_ledger(night, deposits: [P.br_page(transfers), P.br_page([dep])])] },
      { name: 'bitrue-ledger-repeated_page', setup: week, steps: [P.bitrue_ledger(night, deposits: [P.br_page(transfers)])] },
      { name: 'bitrue-ledger-no_permission', setup: week, steps: [P.bitrue_ledger(night, deposits: [err.(401, -2015, "You don't have permission.")], withdrawals: [err.(403, -2015, 'has no operation privilege')])] },
      { name: 'bitrue-ledger-bare_401', setup: week, steps: [P.bitrue_ledger(night, deposits: [err.(401, -1022, 'No login')])] },
      { name: 'bitrue-ledger-symbol_gone', setup: week, steps: [P.bitrue_ledger(night, trades: [err.(400, -1121, 'Invalid symbol.')])] },
      { name: 'bitrue-ledger-symbol_gone_200', setup: week, steps: [P.bitrue_ledger(night, trades: [P.ok('code' => '-1121', 'msg' => 'Invalid symbol.')])] },
      { name: 'bitrue-ledger-account_fails', setup: week, steps: [P.bitrue_ledger(night, account: [err.(401, -1022, 'No login')])] },
      { name: 'bitrue-ledger-listing_fails', setup: week, steps: [P.bitrue_ledger(night, info: [err.(503, -1000, 'System abnormality')])] },
      { name: 'bitrue-ledger-trades_fail', setup: week, steps: [P.bitrue_ledger(night, trades: [err.(503, -1000, 'System abnormality')])] },
      { name: 'bitrue-ledger-trades_envelope', setup: week, steps: [P.bitrue_ledger(night, trades: [P.ok('code' => 500, 'msg' => 'System abnormality')])] },
      { name: 'bitrue-ledger-transfers_not_object', setup: week, steps: [P.bitrue_ledger(night, deposits: [P.ok([])])] },
      { name: 'bitrue-ledger-envelope', setup: week, steps: [P.bitrue_ledger(night, deposits: [P.ok('code' => 500, 'msg' => 'System abnormality')])] },
      { name: 'bitrue-ledger-sold_out', setup: lambda { |ctx|
          week.call(ctx)
          AccountTransaction.create!(user: ctx[:user], api_key: ctx[:key], exchange: ctx[:bitrue], entry_type: :sell, base_currency: 'ETH', base_amount: 1,
                                     quote_currency: 'USDT', quote_amount: 2000, transacted_at: at - 10.days, tx_id: 'old')
        }, steps: [P.bitrue_ledger(night, account: [P.mx_account], info: [P.mx_info(%w[BTC USDT], %w[ETH USDT])], trades: [P.ok(trades)])] },
      { name: 'bitrue-ledger-bot_order_pair', setup: lambda { |ctx|
          week.call(ctx)
          bot = P.holder(ctx, 'ETH', at: at - 10.days, venue: ctx[:bitrue])
          bot.transactions.update_all(quote: 'USDT', quote_asset_id: ctx[:assets]['USDT'].id)
        }, steps: [P.bitrue_ledger(night, account: [P.mx_account], info: [P.mx_info(%w[BTC USDT], %w[ETH USDT])], trades: [P.ok(trades)])] },
      { name: 'bitrue-ledger-nothing', setup: week, steps: [P.bitrue_ledger(night)] }
    ] + {
      'null' => nil, 'array' => [], 'missing_symbols' => {},
      'null_symbols' => { 'symbols' => nil }, 'empty_symbols' => { 'symbols' => [] },
      'empty_object_symbols' => { 'symbols' => {} }, 'object_symbols' => { 'symbols' => { 'bad' => 1 } },
      'string_symbols' => { 'symbols' => 'bad' }, 'number_symbols' => { 'symbols' => 1 },
      'false_symbols' => { 'symbols' => false }
    }.map do |name, body|
      { name: "bitrue-ledger-listing_#{name}", setup: week,
        steps: [P.bitrue_ledger(night, info: [P.ok(body)], trades: [P.ok(trades)], deposits: [P.br_page([dep])])] }
    end
  end

  def bitrue_balance_scenarios
    night = '2026-09-20T02:30:00.750000Z'
    install = ->(ctx) { P.bitrue(ctx) }
    err = ->(status, code, msg) { { 'status' => status, 'body' => { 'code' => code, 'msg' => msg } } }
    [
      { name: 'bitrue-balances-lowercase', setup: install, steps: [P.bitrue_balances(night, account: P.mx_account(%w[btc 0.5 0.1], %w[eth 2], %w[usdt 100.5]), prices: P.prices_body('bitcoin' => 60000, 'ethereum' => 2500))] },
      { name: 'bitrue-balances-invalid_key', setup: install, steps: [P.bitrue_balances(night, account: err.(401, -1022, 'No login'))] },
      { name: 'bitrue-balances-no_permission', setup: install, steps: [P.bitrue_balances(night, account: err.(403, -2015, "You don't have permission."))] },
      { name: 'bitrue-balances-throttle', setup: install, steps: [P.bitrue_balances(night, account: err.(429, -1003, 'Too many requests.'))] },
      { name: 'bitrue-balances-nothing', setup: install, steps: [P.bitrue_balances(night, account: P.mx_account)] }
    ]
  end

  def all = ledger_scenarios + balance_scenarios + binance_balance_scenarios + binance_ledger_scenarios + import_scenarios + kucoin_ledger_scenarios +
            kucoin_balance_scenarios + bitvavo_ledger_scenarios + bitvavo_balance_scenarios + gemini_ledger_scenarios + gemini_balance_scenarios +
            bitget_ledger_scenarios + bitget_balance_scenarios + mexc_ledger_scenarios + mexc_balance_scenarios + hyperliquid_ledger_scenarios + hyperliquid_balance_scenarios + bybit_ledger_scenarios + bybit_balance_scenarios + bitrue_ledger_scenarios + bitrue_balance_scenarios + bingx_ledger_scenarios + bingx_balance_scenarios
end
