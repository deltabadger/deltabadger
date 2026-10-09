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

  def all = ledger_scenarios + balance_scenarios
end
