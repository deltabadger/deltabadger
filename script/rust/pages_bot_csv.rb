# 3.0 A8 (PLAN-A8-bot-csv): the bot's CSV export and import (Bots::ExportsController, Bots::ImportsController,
# Bot::Exportable). Every POST is an action snapshot, so the rows an import writes are compared with Rails' own. The token
# comes from the settings page (the bot page of a history that sold is not served by these pages). No scenario holds a
# waiting row: Rails' export would ask the venue for it first (D-EXPORT-NO-REFRESH), and the grid has no network.
# Loaded by pages.rb.
module Pages
  module BotCsvGrid
    # 'csv_spellings' => { symbol => spelling }: the bot's venue lists that member under another name (Ticker#base).
    def bot(spec)
      record = super
      (spec['csv_spellings'] || {}).each do |symbol, spelling|
        Ticker.where(exchange: record.exchange, base_asset_id: asset_id(symbol)).update_all(base: spelling)
      end
      record
    end
  end
  singleton_class.prepend(BotCsvGrid)

  module_function

  CSV_HEADER = 'Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status'.freeze

  def csv_file(*rows) = ([CSV_HEADER] + rows).join("\n") + "\n"

  def bot_csv_scenarios
    fill = { 'price' => '412.37', 'amount' => '0.121250333', 'amount_exec' => '0.121250333', 'quote_amount' => '50', 'quote_amount_exec' => '49.999999' }
    # Every cell the export writes: a formula-looking id, the fallbacks (amount for a missing executed amount, amount*price
    # for a missing value, blank for a price of zero or below), explicit zeros that win over them, rows it leaves out (failed,
    # cancelled), and a merged row placed on Interactive Brokers (exchange 2).
    history = [
      ['QQQM', '2026-09-08T13:30:01Z', fill.merge('external_id' => '=1+1')],
      ['QQQM', '2026-09-09T13:30:01Z', fill.merge('external_id' => '-5', 'side' => 'sell', 'price' => '420', 'amount' => '0.1', 'amount_exec' => nil,
                                                  'quote_amount' => nil, 'quote_amount_exec' => nil)],
      ['QQQM', '2026-09-09T14:00:00Z', fill.merge('external_id' => 'zero-price', 'price' => '0', 'quote_amount' => nil, 'quote_amount_exec' => nil)],
      ['QQQM', '2026-09-09T14:30:00Z', fill.merge('external_id' => 'negative-price', 'price' => '-1', 'quote_amount' => nil, 'quote_amount_exec' => nil)],
      ['QQQM', '2026-09-09T15:00:00Z', fill.merge('external_id' => 'zeros', 'amount_exec' => '0', 'quote_amount_exec' => '0')],
      ['QQQM', '2026-09-09T15:30:00Z', fill.merge('external_id' => nil, 'status' => 'failed', 'external_status' => nil)],
      ['QQQM', '2026-09-09T16:00:00Z', fill.merge('external_id' => 'cancelled', 'external_status' => 'cancelled', 'order_type' => 'limit_order')],
      ['QQQM', '2026-09-09T16:30:00Z', fill.merge('external_id' => 'ib-1', 'exchange_id' => 2, 'order_type' => 'limit_order')],
      ['QQQM', '2026-09-10T09:00:00.654321Z', fill.merge('external_id' => 'late, "quoted"')]
    ]
    basket = [['QQQM', '2026-09-08T13:30:01Z', fill.merge('external_id' => 'b-1')],
              ['IBIT', '2026-09-08T13:30:02Z', fill.merge('external_id' => 'b-2', 'price' => '50', 'amount' => '0.4', 'amount_exec' => '0.4')]]
    token = get('/settings/account')
    export = ->(path, expect) { post(path, {}, 'csrf' => 'header', 'expect' => expect, 'action_snapshot' => true, 'may_mutate' => [], 'headers_extra' => ['content-disposition']) }
    import = lambda do |path, content, expect = 302, filename: 'orders.csv'|
      post(path, {}, 'csrf' => 'header', 'expect' => expect, 'action_snapshot' => true, 'may_mutate' => ['transactions'])
        .merge('multipart' => [{ 'name' => 'file', 'filename' => filename, 'content' => content }])
    end
    shown = ->(path = '/settings/account') { get(path).merge('expect' => 200) }
    row = ->(at, id, side, amount, value, price, base, quote: 'USD', status: 'Closed', type: 'Market') { [at, id, type, side, amount, value, price, base, quote, status].join(',') }
    one = csv_file(row.('2026-09-08 16:30:01 +0300', 'o1', 'Buy', '0.12', '49.5', '412.5', 'QQQM'),
                   row.('2026-09-09 13:30:01 UTC', '=2+2', 'Buy', '0.2', '82', '410', 'QQQM'))
    {
      'bot_csv_export_single' => with_bots([stopped('single', 'orders' => history)], signed_in(token, export.('/bots/1/export', 200))),
      'bot_csv_export_utc' => with_bots([stopped('single', 'orders' => history)], signed_in(token, export.('/bots/1/export', 200)), 'user' => owner('time_zone' => 'UTC')),
      'bot_csv_export_london' => with_bots([stopped('single', 'orders' => history)], signed_in(token, export.('/bots/1/export', 200)), 'user' => owner('time_zone' => 'London')),
      'bot_csv_export_basket' => with_bots([stopped('basket', 'orders' => basket)], signed_in(token, export.('/bots/1/export', 200))),
      'bot_csv_export_index' => with_bots([stopped('index', 'orders' => 'index_rotation')], signed_in(token, export.('/bots/1/export', 200))),
      'bot_csv_export_signal' => with_bots([{ 'kind' => 'signal', 'columns' => { 'status' => 2 }, 'orders' => 'settled' }], signed_in(token, export.('/bots/1/export', 200))),
      'bot_csv_export_empty' => with_bots([stopped('single')], signed_in(token, export.('/bots/1/export', 200))),
      'bot_csv_export_de' => with_bots([stopped('single', 'orders' => basket.first(1))], signed_in(token, export.('/de/bots/1/export', 200))),
      'bot_csv_export_missing' => with_bots([stopped('single')], signed_in(token, export.('/bots/99/export', 302), get('/bots').merge('expect' => 302))),
      'bot_csv_export_deleted' => with_bots([stopped('single', 'columns' => { 'status' => 3 })], signed_in(token, export.('/bots/1/export', 302), shown.('/bots'))),
      'bot_csv_export_signed_out' => with_bots([stopped('single')], [get('/login'), export.('/bots/1/export', 302).except('action_snapshot', 'may_mutate')]),

      'bot_csv_import_single' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', one), shown.())),
      'bot_csv_import_again' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', one), shown.(), import.('/bots/1/import', one), shown.())),
      'bot_csv_import_second_bot' => with_bots([stopped('single'), stopped('single')],
                                               signed_in(token, import.('/bots/1/import', one), shown.(), import.('/bots/2/import', one), shown.())),
      'bot_csv_import_basket' => with_bots([stopped('basket')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 +0000', 'b-1', 'Buy', '0.12', '49.5', '412.5', 'QQQM'), row.('2026-09-08 13:30:02 +0000', 'b-2', 'Buy', '0.4', '20', '50', 'IBIT',
                                                                                              type: 'Limit'))), shown.())),
      # An index bot: a member (NVDA) and a stock its venue lists that is no member (QQQM), each resolved to its one asset.
      'bot_csv_import_index' => with_bots([stopped('index')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'i-1', 'Buy', '0.1', '40', '400', 'NVDA'), row.('2026-09-08 13:30:02 UTC', 'i-2', 'Buy', '0.1', '41', '410', 'QQQM'))), shown.())),
      'bot_csv_import_signal' => with_bots([{ 'kind' => 'signal', 'columns' => { 'status' => 2 } }], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 's-1', 'Buy', '0.1', '40', '400', 'QQQM'), row.('2026-09-08 14:30:01 UTC', 's-2', 'Sell', '0.05', '21', '420', 'QQQM'))), shown.())),
      # Kraken names bitcoin XBT; the bot's quote is EUR.
      'bot_csv_import_venue_spelling' => with_bots([stopped('kraken')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'k-1', 'Buy', '0.001', '50', '50000', 'XBT', quote: 'EUR'),
        row.('2026-09-08 13:30:02 UTC', 'k-2', 'Buy', '0.001', '51', '51000', 'btc', quote: 'eur'))), shown.()), 'venues' => true),
      'bot_csv_import_wrong_header' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', "Date,Amount\n2026-09-08,1\n"), shown.())),
      'bot_csv_import_malformed' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', "#{CSV_HEADER}\n\"unclosed,1\n"), shown.())),
      'bot_csv_import_no_file' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', '', filename: ''), shown.())),
      'bot_csv_import_quote_mismatch' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'q-1', 'Buy', '0.1', '40', '400', 'QQQM', quote: 'EUR'))), shown.())),
      'bot_csv_import_missing' => with_bots([stopped('single')], signed_in(token, import.('/bots/99/import', one), get('/bots').merge('expect' => 302))),
      'bot_csv_import_signed_out' => with_bots([stopped('single')], [get('/login'), import.('/bots/1/import', one).except('action_snapshot', 'may_mutate')]),
      'bot_csv_import_de' => with_bots([stopped('single')], signed_in(token, import.('/de/bots/1/import', one), shown.('/de/settings/account'),
                                                                      import.('/de/bots/1/import', "Date\n"), shown.('/de/settings/account'))),
      'bot_csv_import_pl' => with_bots([stopped('single')], signed_in(token, import.('/pl/bots/1/import', one), shown.('/pl/settings/account'),
                                                                      import.('/pl/bots/1/import', "Date\n"), shown.('/pl/settings/account'))),
      # Listed divergences (rust/tests/pages.rs `bot_csv_difference`).
      # D-IMPORT-COUNTS: rows not closed are skipped uncounted by Rails.
      'bot_csv_import_not_closed' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'n-1', 'Buy', '0.1', '40', '400', 'QQQM', status: 'Open'),
        row.('2026-09-08 13:30:02 UTC', 'n-2', 'Buy', '0.1', '40', '400', 'QQQM', status: 'Cancelled'),
        row.('2026-09-08 13:30:03 UTC', 'n-3', 'Buy', '0.1', '40', '400', 'QQQM'))), shown.())),
      # D-IMPORT-COUNTS: IBIT's listing spelled QQQM, so QQQM names two members; Rails counts the skip and never shows it.
      'bot_csv_import_ambiguous' => with_bots([stopped('basket', 'csv_spellings' => { 'IBIT' => 'QQQM' })], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'a-1', 'Buy', '0.1', '40', '400', 'QQQM'), row.('2026-09-08 13:30:02 UTC', 'a-2', 'Buy', '0.4', '20', '50', 'IBIT'))), shown.())),
      # R-IMPORT-INDEX-NO-ASSET: a name neither the members nor the venue know.
      'bot_csv_import_index_unlisted' => with_bots([stopped('index')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'u-1', 'Buy', '0.1', '40', '400', 'ZZZZ'), row.('2026-09-08 13:30:02 UTC', 'u-2', 'Buy', '0.1', '40', '400', 'NVDA'))), shown.())),
      # D-IMPORT-DUP: the same id twice in one file; Rails' insert_all skips the second and counts it imported.
      'bot_csv_import_duplicate' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'd-1', 'Buy', '0.1', '40', '400', 'QQQM'), row.('2026-09-08 13:30:02 UTC', 'd-1', 'Buy', '0.2', '80', '400', 'QQQM'))), shown.())),
      # D-IMPORT-REIMPORT: a bot's own export (its rows already imported) imported again into it.
      'bot_csv_import_own_export' => with_bots([stopped('single')], signed_in(token, import.('/bots/1/import', one), shown.(), import.('/bots/1/import', csv_file(
        row.('2026-09-08 13:30:01 UTC', 'imported_1_o1', 'Buy', '0.12', '49.5', '412.5', 'QQQM'))), shown.()))
    }.tap { |grid| raise "expected 31 bot CSV scenarios, got #{grid.size}" unless grid.size == 31 }
  end
end
