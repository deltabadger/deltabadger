# Binance's own transaction export, read into the shapes `Exchanges::Binance#get_ledger` produces.
#
# Why it exists: Binance serves roughly 90 days through `myTrades` and the deposit/withdraw
# endpoints, so an account older than that has funding history nothing can fetch — which is what
# leaves the tracker inferring how much money went in, and the chart comparing a real portfolio
# against a guess. The export goes back to the beginning.
#
# The mapping is not free-standing. Every entry here has to match what the adapter would have made
# of the same event, field for field: a file overlaps the window the API already synced, and a row
# with no id dedups on (api_key, entry_type, base_currency, base_amount, transacted_at). Disagree
# with the adapter about an entry type, an amount's sign, or a fee's home, and the overlap doubles
# instead of deduping. That is why `swap` and `buy` are decided by the same cash-leg test the
# adapter uses, and why dust becomes the same swap_out/swap_in pair rather than one tidy row.
#
# Transfers are the exception that cannot be mapped away: the export dates a deposit when it was
# credited and writes a withdrawal gross at its completion, and neither is in the API's row. Their
# `raw_data` keeps the operation, and AccountTransactionSync pairs them on that (see "one transfer").
class Import::BinanceCsv
  # Every operation the export writes, and what it is. Absent from this table means we do not know —
  # such a row is reported, never guessed at, because a wrong guess becomes a wrong tax figure.
  #
  # `nil` is a DELIBERATE skip: money moving between the user's own Binance wallets is not money
  # arriving, and counting it as funding would inflate the very figure this import exists to fix.
  OPERATIONS = {
    'Transaction Buy' => :trade_in, 'Transaction Spend' => :trade_out,
    'Transaction Sold' => :trade_out, 'Transaction Revenue' => :trade_in,
    'Transaction Fee' => :trade_fee,
    'Buy' => :trade_in, 'Sell' => :trade_out, 'Fee' => :trade_fee,
    'Binance Convert' => :convert,
    'Small Assets Exchange BNB' => :dust,
    'Deposit' => :deposit, 'Fiat Deposit' => :deposit,
    'Withdraw' => :withdrawal, 'Fiat Withdraw' => :withdrawal,
    'Commission Rebate' => :other_income,
    'Commission History' => :other_income,
    'Referral Kickback' => :other_income,
    'Commission Fee Shared With You' => :other_income,
    'Distribution' => :other_income,
    'Asset Recovery' => :other_income,
    # Seen only as a clawback ("TWT回收", a staking reward taken back), which the sign makes `lost`.
    'Asset - Transfer' => :other_income,
    'Token Swap - Redenomination/Rebranding' => :other_income,
    'Pool Distribution' => :staking_reward,
    'Staking Rewards' => :staking_reward,
    'Simple Earn Flexible Interest' => :lending_interest,
    'Simple Earn Locked Rewards' => :lending_interest,
    'Savings Interest' => :lending_interest,
    'Airdrop Assets' => :airdrop,
    # The API books this as a dividend, which `import_dividends` makes an airdrop.
    'Token Swap - Distribution' => :airdrop,
    'Transfer Between Spot and Funding' => nil,
    'Transfer Between Main and Funding Account' => nil,
    'Ledger - Fund Migration' => nil,
    'transfer_in' => nil, 'transfer_out' => nil
  }.freeze

  # The stable-value legs. A trade against one of these is a purchase or a sale; a trade against
  # anything else is a swap between two positions — the same split `normalize_trade` makes.
  CASH = /\A(USDT|USDC|BUSD|DAI|FDUSD|TUSD|USD|EUR|GBP|CHF|SEK|PLN|DKK|CZK|BGN|AUD|CAD|JPY|AED|TRY|BRL|RUB|UAH|NGN|ZAR)\z/

  # Binance names the file after the zone it exported in; the rows themselves state a wall clock and
  # no zone at all. Read at the wrong offset, every row misses what is already stored and the whole
  # history lands a second time — so the filename is the best evidence available and the user can
  # still override it.
  FILENAME_OFFSET = /\(UTC([+-])(\d{1,2})(?::([0-5]\d))?\)/

  # Older exports name the time column `UTC_Time` and write it in UTC, with no zone in the filename.
  # The header IS the zone there, so such a file needs no name to read it by.
  UTC_TIME = 'UTC_Time'.freeze

  attr_reader :unrecognised

  # Where a file in this format can possibly have come from. Binance.US subclasses Exchanges::Binance
  # and writes the identical export, so one type covers both — and nothing else on earth writes it.
  # This is what makes the account question answerable without asking: choosing the format IS
  # choosing the venue.
  def self.exchange_types = [Exchanges::Binance]

  # The rows state a wall clock and no zone. Without one, every timestamp is a guess.
  def self.requires_offset? = true

  def self.offset_from(filename, text = nil)
    return '+00:00' if utc_header?(text)

    match = FILENAME_OFFSET.match(filename.to_s)
    return nil unless match

    # The sign is its own capture: "-0:30" read as the integer -0 is +0, and the offset flipped.
    hours = match[2].to_i
    minutes = match[3].to_i
    sign = hours.zero? && minutes.zero? ? '+' : match[1]
    format('%<sign>s%<hours>02d:%<minutes>02d', sign: sign, hours: hours, minutes: minutes)
  end

  def self.utc_header?(text)
    Array(CSV.parse_line(Import::Run.as_text(text))).include?(UTC_TIME)
  rescue CSV::MalformedCSVError
    false
  end

  def initialize(text, offset: '+00:00')
    # Bytes become text in ONE place, so a byte order mark or a stray non-UTF-8 byte cannot
    # trip a reader constructed directly rather than through the run.
    @text = Import::Run.as_text(text)
    @offset = offset.presence || '+00:00'
    @unrecognised = []
  end

  # Ledger entries in `get_ledger`'s shape, oldest first.
  def entries
    @entries ||= begin
      rows = parse_rows
      trades, singles = rows.partition { |row| %i[trade_in trade_out trade_fee convert dust].include?(row[:kind]) }
      (movement_entries(singles) + grouped_entries(trades))
        .compact.flatten.sort_by { |entry| entry[:transacted_at] }
    end
  end

  private

  def parse_rows
    table = CSV.parse(@text, headers: true)
    utc = table.headers.include?(UTC_TIME)
    table.filter_map do |row|
      operation = row['Operation'].to_s.strip
      next if operation.blank?

      unless OPERATIONS.key?(operation)
        @unrecognised << operation unless @unrecognised.include?(operation)
        next
      end
      kind = OPERATIONS[operation]
      next if kind.nil? # a deliberate skip, not an unknown

      { kind: kind, operation: operation, coin: row['Coin'].to_s.strip,
        change: Import::Run.amount(row['Change']), at: parse_time(utc ? row[UTC_TIME] : row['Time'], utc), account: row['Account'].to_s.strip,
        remark: row['Remark'].to_s.strip }
    end
  end

  def parse_time(value, utc)
    Import::Run.time(value, utc ? '+00:00' : @offset)
  end

  # An operation NAME does not say which way an asset moved; the `Change` column does. Binance takes
  # a delisted token back under the same name it uses to hand tokens out ("Asset Recovery"), and
  # renames a token by removing one and adding another in the same second ("Token Swap -
  # Redenomination/Rebranding"). Reading the name alone books a loss as income and a rename as two
  # windfalls — the coins then never leave the ledger, and it comes to hold what the venue does not.
  def movement_entries(rows)
    rows.group_by { |row| [row[:at], row[:operation]] }.flat_map do |(at, _operation), group|
      out, inn = group.partition { |row| row[:change].negative? }
      next swap_pairs(at, out, inn) if out.any? && inn.any?

      group.map { |row| single_entry(row) }
    end
  end

  # One asset becoming another. Paired by size for the same reason the trade legs are; when the two
  # sides differ in count (two tokens merged into one), the rename is one swap holding every leg.
  def swap_pairs(at, out, inn)
    out = out.sort_by { |row| row[:change] }
    inn = inn.sort_by { |row| -row[:change] }
    return swap_group(at, out, inn, []) unless out.size == inn.size

    out.zip(inn).map { |sold, bought| swap(at, sold, bought, nil) }
  end

  def single_entry(row)
    amount = row[:change].abs
    return if amount.zero?

    base_entry(row).merge(entry_type: outward_type(row), base_amount: amount)
  end

  # A negative change is the asset leaving. `withdrawal` already says that; anything else that was
  # mapped as income has to say it too, and `lost` is the ledger's word for an asset simply gone.
  def outward_type(row)
    return row[:kind] if row[:change].positive? || row[:kind] == :withdrawal

    :lost
  end

  # A trade is however many rows Binance chose to write for it, sharing one timestamp. They are
  # regrouped by (time, account) and then read as one event.
  #
  # A CONVERT is the exception: Binance writes its two halves up to a second apart, so grouped by
  # exact timestamp they never meet and a half with nothing to pair against is dropped — both
  # halves, in fact. They are collected across the whole file and paired by proximity instead.
  def grouped_entries(rows)
    converts, rest = rows.partition { |row| row[:kind] == :convert }
    convert_entries(converts) + rest.group_by { |row| [row[:at], row[:account]] }.flat_map do |(at, _account), group|
      dust = group.select { |row| row[:kind] == :dust }
      trade = group - dust
      dust_entries(at, dust) + trade_entries(at, trade).flatten
    end
  end

  # How far apart a Convert's two halves may be written. One second is what a real export shows; a
  # minute is slack enough for that and far too short to join two separate converts.
  CONVERT_WINDOW = 60

  # Each spend paired with the nearest credit that has not been taken. Timestamped at the spend, so
  # the row sits where the money left.
  #
  # Always a SWAP pair, cash leg or not: `import_convert_trades` books every Convert as swap_out and
  # swap_in without asking what was spent, and a file row dedups against a stored row only when the
  # entry type matches — a Convert out of USDT read as a purchase never met the API's swap, and the
  # coins arrived twice. Priced the same way as the API's, by the cash leg beside the coin leg.
  #
  # A half with no partner in the window (the other half fell outside the export) is NOT imported but
  # reported: stored alone it would be an orphan leg at its own time, which the complete export —
  # stamping the pair at the spend — could neither pair with nor dedup against. The next export
  # covering both halves imports the pair whole.
  def convert_entries(rows)
    spent, received = rows.partition { |row| row[:change].negative? }
    received = received.sort_by { |row| row[:at] }
    spent.sort_by { |row| row[:at] }.filter_map do |out|
      match = received.find { |row| (row[:at] - out[:at]).abs <= CONVERT_WINDOW }
      next lone_convert(out) unless match

      received.delete(match)
      swap(out[:at], out, match, nil)
    end.flatten + received.filter_map { |row| lone_convert(row) }
  end

  def lone_convert(row)
    note = "#{row[:operation]} #{row[:coin]} #{row[:change].to_s('F').delete_suffix('.0')} #{row[:at].strftime('%Y-%m-%d %H:%M:%S')} UTC"
    @unrecognised << note unless @unrecognised.include?(note)
    nil
  end

  # A dust sweep is many coins into BNB at one timestamp, one row per leg. The `Remark` — "CHZ to
  # BNB" — is what says which BNB credit came from which coin, and it is the only thing that does.
  # Read one for one, because `import_dust_conversions` emits a pair PER COIN: collapse a batch into
  # a single receipt and every dust row inside the API's window is unmatched, so it doubles.
  #
  # A batch with no remarks to pair on (an older export, a renamed column) falls back to one pair
  # against the summed receipt — still the right quantities, just one swap instead of many.
  def dust_entries(at, rows)
    return [] if rows.empty?
    return dust_batch(at, rows, 'dustcsv') if rows.any? { |row| row[:remark].blank? }

    rows.group_by { |row| row[:remark] }.flat_map do |remark, legs|
      dust_batch(at, legs, "dustcsv_#{remark}")
    end
  end

  def dust_batch(at, rows, key)
    spent, received = rows.partition { |row| row[:change].negative? }
    # Binance does not always write the coins a sweep consumed. The credits still arrived, so they
    # are booked as arriving rather than dropped — a swap with nothing on the other side would claim
    # a cost basis the file never states, and dropping them leaves the ledger short of BNB the
    # account really holds.
    return received.map { |row| single_entry(row.merge(kind: :other_income)) } if spent.empty?

    swap_legs(at, spent, received, "#{key}_#{at.to_i}", [])
  end

  # One trade per FILL. Binance's matching engine fills an order in pieces and writes a Spend/Buy/Fee
  # triple for each, all sharing one second — reading the group as a single trade keeps one piece and
  # silently drops the rest, so coins that left the account stay in the ledger forever.
  #
  # The rows arrive interleaved and carry no order id, so the legs are paired BY SIZE: fills of one
  # order share a price, which is what makes the largest spend belong to the largest buy. Verified
  # against a real export — pairing in file order instead prices a 0.00005 BTC fill at a 0.00003 BTC
  # fill's cost, and the two agree to the cent when sorted.
  def trade_entries(at, rows)
    fees = rows.select { |row| row[:kind] == :trade_fee }
    legs = rows - fees
    incoming = legs.select { |row| row[:change].positive? }.sort_by { |row| -row[:change] }
    outgoing = legs.select { |row| row[:change].negative? }.sort_by { |row| row[:change] }
    return one_sided(rows) if incoming.empty? || outgoing.empty?

    # More than one coin on a side is unrelated trades sharing a second, and nothing in the file says
    # which leg belongs to which. Never seen in a real export; if it appears, the group is ONE swap
    # holding every leg under its own coin and amount, rather than legs summed under one coin's name.
    return swap_group(at, outgoing, incoming, fees) if mixed_coins?(outgoing, incoming)

    outgoing, incoming = even_up(outgoing, incoming)
    slots, extra = spread_fees(at, fees, incoming.map { |row| row[:change] })
    outgoing.zip(incoming, slots).map { |spent, bought, fee| trade(at, spent, bought, fee) } + extra
  end

  # A trade with nothing on its other side states no price and no counterpart: booking it would
  # invent one. Reported, row by row, so the user sees what did not land rather than losing it.
  def one_sided(rows)
    rows.each do |row|
      note = "#{row[:operation]} #{row[:coin]} #{row[:at].strftime('%Y-%m-%d %H:%M:%S')} UTC"
      @unrecognised << note unless @unrecognised.include?(note)
    end
    []
  end

  def mixed_coins?(outgoing, incoming)
    outgoing.map { |row| row[:coin] }.uniq.size > 1 || incoming.map { |row| row[:coin] }.uniq.size > 1
  end

  # One pair in one second is one price. Where Binance wrote fewer rows on one side than fills on the
  # other (one cash row for two buys), that side is summed and split across the other side's rows by
  # quantity — so a cash trade stays one buy or sell row per fill, the shape the API stores and
  # dedups against, and no leg is reused for a second fill.
  def even_up(outgoing, incoming)
    return [outgoing, incoming] if outgoing.size == incoming.size

    many, few = outgoing.size > incoming.size ? [outgoing, incoming] : [incoming, outgoing]
    total = few.sum { |row| row[:change] }
    split = split(total, many.map { |row| row[:change].abs }).map { |amount| { coin: few.first[:coin], change: amount } }
    many.equal?(outgoing) ? [outgoing, split] : [split, incoming]
  end

  # `total` in parts proportional to `weights`, to 8 places (Binance's finest) or the total's own,
  # cut toward zero; the last part takes the remainder, so the parts sum to the total exactly.
  def split(total, weights)
    places = [8, total.to_s('F').split('.').last.size].max
    sum = weights.sum
    parts = weights[0...-1].map { |weight| (total * weight / sum).round(places, BigDecimal::ROUND_DOWN) }
    parts + [total - parts.sum]
  end

  # [a fee per slot, the fee rows no slot could hold], one slot per weight. A fee row per slot,
  # largest with largest — the same reasoning as the legs. Fees in ONE coin that do not match the
  # slots are split across them by weight, like a cash leg. Otherwise the fees are summed PER COIN,
  # one coin to a slot, and a coin left over (more fee coins than slots) is booked as a fee of its
  # own: never added to another coin's amount, never lost.
  def spread_fees(at, fees, weights)
    slots = weights.size
    return [Array.new(slots), []] if fees.empty?

    sorted = fees.sort_by { |row| row[:change] }
    return [sorted, []] if sorted.size == slots

    coin = sorted.first[:coin]
    if sorted.all? { |row| row[:coin] == coin }
      return [split(sorted.sum { |row| row[:change] }, weights.map(&:abs)).map { |amount| { coin: coin, change: amount } }, []]
    end

    per_coin = sorted.group_by { |row| row[:coin] }.map do |coin, rows|
      { coin: coin, change: rows.sum { |row| row[:change] } }
    end
    extra = per_coin.drop(slots).map { |fee| leg(at, fee[:coin], fee[:change].abs, :fee, nil) }
    [per_coin.first(slots) + Array.new([slots - per_coin.size, 0].max), extra]
  end

  # Legs that cannot be paired: one swap, every leg its own row. Fees ride on the incoming legs, where
  # `swap` puts a fill's fee.
  def swap_group(at, outgoing, incoming, fees)
    coins = ->(rows) { rows.map { |row| row[:coin] }.uniq.join('-') }
    swap_legs(at, outgoing, incoming, "swapcsv_#{at.to_i}_#{coins.call(outgoing)}_#{coins.call(incoming)}", fees)
  end

  def swap_legs(at, spent, received, group_id, fees)
    slots, extra = spread_fees(at, fees, received.map { |row| row[:change] })
    spent.map { |row| leg(at, row[:coin], row[:change].abs, :swap_out, group_id) } +
      received.zip(slots).map { |row, fee| leg(at, row[:coin], row[:change], :swap_in, group_id).merge(fee_of(fee)) } +
      extra
  end

  # Which of the three a fill is, decided by which side holds the cash — the same split
  # `normalize_trade` makes, so an overlap with the API dedups instead of doubling.
  def trade(at, spent, bought, fee)
    return purchase(at, bought, spent, fee) if spent[:coin].match?(CASH)
    return sale(at, spent, bought, fee) if bought[:coin].match?(CASH)

    swap(at, spent, bought, fee)
  end

  def purchase(at, bought, spent, fee)
    leg(at, bought[:coin], bought[:change], :buy, nil)
      .merge(quote_currency: spent[:coin], quote_amount: spent[:change].abs)
      .merge(fee_of(fee))
  end

  def sale(at, sold, received, fee)
    leg(at, sold[:coin], sold[:change].abs, :sell, nil)
      .merge(quote_currency: received[:coin], quote_amount: received[:change])
      .merge(fee_of(fee))
  end

  # No cash on either side: two positions, so two rows sharing a group — `normalize_trade`'s own
  # shape. The fee rides on the incoming leg, where the adapter puts it.
  def swap(at, sold, bought, fee)
    group_id = "swapcsv_#{at.to_i}_#{sold[:coin]}_#{bought[:coin]}"
    [leg(at, sold[:coin], sold[:change].abs, :swap_out, group_id),
     leg(at, bought[:coin], bought[:change], :swap_in, group_id).merge(fee_of(fee))]
  end

  def fee_of(fee)
    return {} unless fee

    { fee_currency: fee[:coin], fee_amount: fee[:change].abs }
  end

  def leg(at, coin, amount, entry_type, group_id)
    { entry_type: entry_type, base_currency: coin, base_amount: amount,
      quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
      tx_id: nil, group_id: group_id, description: nil, transacted_at: at, raw_data: {} }
  end

  def base_entry(row)
    leg(row[:at], row[:coin], row[:change].abs, row[:kind], nil)
      .merge(description: row[:operation], raw_data: { 'operation' => row[:operation] })
  end
end
