# Records what rust/src/tracker/import must reproduce of Rails' CSV import readers, from the app's own code.
#   bin/rails runner script/rust/record_import_vectors.rb rust/tests/fixtures/import_vectors.json
# Every file is SYNTHETIC: User ID 1, made-up coins, amounts and remarks; no real export goes in here. Each case goes
# through the controller's offset step (Import::BinanceCsv.offset_from on the filename), Import::Run.as_text, the
# reader's #entries and #unrecognised, and Import::Run#import! for the error it answers (:unreadable / :malformed), with a
# user who holds no keys so nothing is written. Re-run when a file under "ported_sources" changes (script/rust/repin).
require 'json'
require 'digest'
require 'base64'
require 'active_support/testing/time_helpers'

ENV['TZ'] = 'UTC' # Deltabadger dates without a zone are read in the process zone; pin it.
# Pinned: Time.parse fills what a value lacks from the clock; Import::Run.time refuses such a cell, and a regression shows here.
NOW = Time.utc(2026, 10, 1, 21, 30, 0)
include ActiveSupport::Testing::TimeHelpers
travel_to(NOW)

HEADER = 'User ID,Time,Account,Operation,Coin,Change,Remark'.freeze
UTC0 = 'Binance-Transaction-History-202601010000(UTC+0)-part1-of1.csv'.freeze
UTC3 = 'Binance-Transaction-History-202601010000(UTC+3)-part1-of1.csv'.freeze
BOM = "\xEF\xBB\xBF".b.freeze

user = User.find_or_initialize_by(email: 'owner@example.com')
user.assign_attributes(name: 'Owner', password: 'Correct-horse-battery-staple-1', confirmed_at: Time.current)
user.save!(validate: false)
raise 'the recorder needs a user with no keys, or a run would write' if user.api_keys.any?

raw_text = ->(raw) { AccountTransaction.new(raw_data: raw).instance_variable_get(:@attributes)['raw_data'].value_for_database }
serialize = lambda do |entry|
  entry.to_h do |key, value|
    [key.to_s, case value
               when BigDecimal then value.to_s('F')
               when Time then value.utc.iso8601(6)
               when Symbol then value.to_s
               when Hash then raw_text.(value)
               else value
               end]
  end
end

record = lambda do |name, format, bytes, filename|
  bytes = bytes.b
  reader_class = Import::Run.reader_for(format)
  offset = reader_class.requires_offset? ? Import::BinanceCsv.offset_from(filename, bytes) : nil
  text = Import::Run.as_text(bytes)
  out = { 'name' => name, 'format' => format, 'filename' => filename, 'bytes' => Base64.strict_encode64(bytes),
          'offset' => offset, 'text' => text, 'entries' => [], 'unrecognised' => [], 'error' => nil }
  # The controller refuses a zone-less name before reading a byte.
  next out.merge('error' => 'unknown_timezone') if reader_class.requires_offset? && offset.nil?

  reader = reader_class.new(text, offset: offset)
  begin
    out['entries'] = reader.entries.map(&serialize)
  rescue CSV::MalformedCSVError, ArgumentError => e
    out['exception'] = e.class.name
  rescue StandardError => e # not rescued by Import::Run either: Rails answers 500
    next out.merge('error' => 'raises', 'exception' => e.class.name)
  end
  out['unrecognised'] = reader.unrecognised.dup
  result = Import::Run.new(user: user, format: format, text: bytes, offset: offset).import!
  out['error'] = result[:error].to_s if %i[unreadable malformed].include?(result[:error])
  raise "#{name}: the reader raised #{out['exception']} but the run answered #{result[:error].inspect}" if out['exception'] && out['error'] != 'malformed'

  out
end

bin = ->(name, rows, filename: UTC0, sep: "\n", prefix: ''.b) { [name, 'binance', prefix + ([HEADER] + rows).map(&:b).join(sep.b), filename] }
row = ->(time, op, coin, change, remark = '', account: 'Spot') { "1,#{time},#{account},#{op},#{coin},#{change},#{remark}" }
t0 = '2024-03-01 10:00:00'

binance = [
  # ---- shapes from test/models/import/binance_csv_test.rb ----
  bin.('buy_triple', [row.('2021-07-01 03:42:38', 'Transaction Fee', 'BNB', '-0.00002482'), row.('2021-07-01 03:42:38', 'Transaction Spend', 'USDT', '-10.0003543'),
                      row.('2021-07-01 03:42:38', 'Transaction Buy', 'ZORK', '0.06983')]),
  bin.('sale_triple', [row.('2022-01-05 10:00:00', 'Transaction Sold', 'FLIB', '-0.5'), row.('2022-01-05 10:00:00', 'Transaction Revenue', 'USDT', '21000'),
                       row.('2022-01-05 10:00:00', 'Transaction Fee', 'USDT', '-21')]),
  bin.('swap_no_cash', [row.('2022-03-03 08:00:00', 'Transaction Spend', 'FLIB', '-0.01'), row.('2022-03-03 08:00:00', 'Transaction Buy', 'ZORK', '0.15')]),
  bin.('deposit_withdraw', [row.('2021-07-01 14:07:04', 'Deposit', 'USDT', '150.2'), row.('2021-08-03 17:12:44', 'Withdraw', 'ZORK', '-0.501', 'Withdraw fee is included')]),
  bin.('own_wallet_transfer_only', [row.('2021-09-01 12:00:00', 'Transfer Between Spot and Funding', 'USDT', '-50')]),
  bin.('dust_by_remark', [row.(t0, 'Small Assets Exchange BNB', 'QUUX', '-3', 'QUUX to BNB'), row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.0004', 'QUUX to BNB'),
                          row.(t0, 'Small Assets Exchange BNB', 'BLIP', '-0.0076', 'BLIP to BNB'), row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.00000919', 'BLIP to BNB')]),
  bin.('rebates_rewards', [row.('2021-06-30 17:36:03', 'Commission Rebate', 'FLIB', '0.00000007'), row.('2021-11-02 09:00:00', 'Pool Distribution', 'ZORK', '0.001'),
                           row.('2021-12-02 09:00:00', 'Airdrop Assets', 'QUUX', '0.5')]),
  bin.('two_fills_one_instant', [row.(t0, 'Transaction Buy', 'ZORK', '22.22'), row.(t0, 'Transaction Spend', 'FLIB', '-0.0097768'), row.(t0, 'Transaction Fee', 'BNB', '-0.00072127'),
                                 row.(t0, 'Transaction Spend', 'FLIB', '-0.0020988'), row.(t0, 'Transaction Buy', 'ZORK', '4.77'), row.(t0, 'Transaction Fee', 'BNB', '-0.00015483')]),
  bin.('fills_paired_by_size', [row.(t0, 'Transaction Buy', 'FLIB', '0.00005'), row.(t0, 'Transaction Fee', 'FLIB', '-0.00000005'), row.(t0, 'Transaction Spend', 'USDC', '-3.2167998'),
                                row.(t0, 'Transaction Fee', 'FLIB', '-0.00000003'), row.(t0, 'Transaction Buy', 'FLIB', '0.00003'), row.(t0, 'Transaction Spend', 'USDC', '-5.361333')]),
  bin.('no_trade_row_dropped', [row.(t0, 'Transaction Buy', 'ZORK', '22.22'), row.(t0, 'Transaction Spend', 'FLIB', '-0.0097768'), row.(t0, 'Transaction Spend', 'FLIB', '-0.0020988'),
                                row.(t0, 'Transaction Buy', 'ZORK', '4.77'), row.('2023-01-01 00:00:00', 'Transaction Spend', 'USDT', '-50'),
                                row.('2023-01-01 00:00:00', 'Transaction Buy', 'QUUX', '0.03')]),
  bin.('asset_recovery_negative', [row.('2024-04-26 12:23:15', 'Asset Recovery', 'ZORK', '-31.85')]),
  bin.('asset_recovery_positive', [row.('2024-04-27 12:23:15', 'Asset Recovery', 'ZORK', '31.85')]),
  bin.('token_swap_distribution', [row.('2024-06-26 08:26:41', 'Token Swap - Distribution', 'USDT', '118.55207')]),
  bin.('commission_history', [row.('2021-01-07 21:28:41', 'Commission History', 'USDT', '0.00807839')]),
  bin.('asset_transfer_out_and_in', [row.('2021-01-08 13:59:25', 'Asset - Transfer', 'FLIB', '-100', 'FLIB回收'), row.('2021-01-09 13:59:25', 'Asset - Transfer', 'FLIB', '5')]),
  bin.('token_rebrand', [row.('2022-05-28 22:57:37', 'Token Swap - Redenomination/Rebranding', 'ZORK', '-12.920406'),
                         row.('2022-05-28 22:57:37', 'Token Swap - Redenomination/Rebranding', 'ZORKC', '12.920406')]),
  bin.('token_rebrand_two_out_one_in', [row.(t0, 'Token Swap - Redenomination/Rebranding', 'ZORK', '-1'), row.(t0, 'Token Swap - Redenomination/Rebranding', 'ZORKX', '-3'),
                                        row.(t0, 'Token Swap - Redenomination/Rebranding', 'ZORKC', '4')]),
  bin.('withdrawal_not_lost', [row.('2021-08-03 17:12:44', 'Withdraw', 'ZORK', '-0.501', 'Withdraw fee is included')]),
  bin.('convert_halves_1s_apart', [row.('2025-06-22 20:20:45', 'Binance Convert', 'USDC', '129.41567489'), row.('2025-06-22 20:20:46', 'Binance Convert', 'USDT', '-129.41567489')]),
  bin.('convert_out_of_cash_utc3', [row.('2021-09-20 09:22:54', 'Binance Convert', 'ZORK', '0.89568816'), row.('2021-09-20 09:22:54', 'Binance Convert', 'USDT', '-150')], filename: UTC3),
  bin.('two_converts_far_apart', [row.('2021-09-20 09:22:54', 'Binance Convert', 'ZORK', '0.89568816'), row.('2021-09-20 09:22:54', 'Binance Convert', 'USDT', '-150'),
                                  row.('2024-02-17 20:07:21', 'Binance Convert', 'BNB', '0.00096277'), row.('2024-02-17 20:07:21', 'Binance Convert', 'QUUX', '-3')]),
  bin.('dust_without_spend_leg', [row.('2021-07-06 13:12:11', 'Small Assets Exchange BNB', 'BNB', '0.00271024'), row.('2021-07-06 13:12:11', 'Small Assets Exchange BNB', 'BNB', '0.01097549')]),
  bin.('unknown_operation', [row.('2026-01-01 00:00:00', 'Some Future Binance Product', 'XYZ', '1')]),
  # ---- shapes from test/integration/tracker_import_test.rb ----
  bin.('integration_file_utc3', [row.('2021-07-01 14:07:04', 'Deposit', 'USDT', '150.2'), row.('2021-07-01 03:42:38', 'Transaction Spend', 'USDT', '-10.0003543'),
                                 row.('2021-07-01 03:42:38', 'Transaction Buy', 'ZORK', '0.06983')], filename: UTC3),
  bin.('integration_bom_utc3', [row.('2021-07-01 14:07:04', 'Deposit', 'USDT', '150.2'), row.('2021-07-01 03:42:38', 'Transaction Spend', 'USDT', '-10.0003543'),
                                row.('2021-07-01 03:42:38', 'Transaction Buy', 'ZORK', '0.06983')], filename: UTC3, prefix: BOM),
  bin.('integration_renamed_no_zone', [row.('2021-07-01 14:07:04', 'Deposit', 'USDT', '150.2')], filename: 'renamed.csv'),
  # ---- multi-fill, fee mismatch, mixed coins, accounts ----
  bin.('three_fill_triples', [row.(t0, 'Transaction Spend', 'EUR', '-30'), row.(t0, 'Transaction Buy', 'ZORK', '3'), row.(t0, 'Transaction Fee', 'ZORK', '-0.003'),
                              row.(t0, 'Transaction Spend', 'EUR', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1'), row.(t0, 'Transaction Fee', 'ZORK', '-0.001'),
                              row.(t0, 'Transaction Spend', 'EUR', '-20'), row.(t0, 'Transaction Buy', 'ZORK', '2'), row.(t0, 'Transaction Fee', 'ZORK', '-0.002')]),
  bin.('fee_mismatch_one_fee_two_fills', [row.(t0, 'Transaction Spend', 'USDT', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1'),
                                          row.(t0, 'Transaction Spend', 'USDT', '-20'), row.(t0, 'Transaction Buy', 'ZORK', '2'), row.(t0, 'Transaction Fee', 'BNB', '-0.01')]),
  bin.('fee_mismatch_three_fees_two_fills', [row.(t0, 'Transaction Spend', 'USDT', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1'), row.(t0, 'Transaction Spend', 'USDT', '-20'),
                                             row.(t0, 'Transaction Buy', 'ZORK', '2'), row.(t0, 'Transaction Fee', 'BNB', '-0.01'), row.(t0, 'Transaction Fee', 'BNB', '-0.02'),
                                             row.(t0, 'Transaction Fee', 'USDT', '-0.03')]),
  bin.('more_spends_than_buys', [row.(t0, 'Transaction Spend', 'USDT', '-10'), row.(t0, 'Transaction Spend', 'USDT', '-20'), row.(t0, 'Transaction Buy', 'ZORK', '3')]),
  bin.('mixed_coins_one_second', [row.(t0, 'Transaction Spend', 'USDT', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1'), row.(t0, 'Transaction Spend', 'USDT', '-5'),
                                  row.(t0, 'Transaction Buy', 'FLIB', '2'), row.(t0, 'Transaction Fee', 'BNB', '-0.01'), row.(t0, 'Transaction Fee', 'BNB', '-0.02')]),
  bin.('mixed_coins_no_fee', [row.(t0, 'Transaction Spend', 'FLIB', '-1'), row.(t0, 'Transaction Spend', 'QUUX', '-2'), row.(t0, 'Transaction Buy', 'ZORK', '3')]),
  bin.('one_sided_trade_dropped', [row.(t0, 'Transaction Buy', 'ZORK', '1'), row.(t0, 'Transaction Fee', 'BNB', '-0.01'), row.('2024-03-01 11:00:00', 'Transaction Spend', 'USDT', '-4')]),
  bin.('same_second_two_accounts', [row.(t0, 'Transaction Spend', 'USDT', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1'),
                                    row.(t0, 'Transaction Spend', 'USDT', '-7', account: 'Funding'), row.(t0, 'Transaction Buy', 'FLIB', '2', account: 'Funding')]),
  bin.('legacy_buy_sell_fee', [row.(t0, 'Buy', 'ZORK', '2'), row.(t0, 'Sell', 'GBP', '-50'), row.(t0, 'Fee', 'GBP', '-0.5')]),
  bin.('trade_against_lowercase_cash', [row.(t0, 'Transaction Spend', 'usdt', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1')]),
  # ---- converts ----
  bin.('convert_halves_61s_apart', [row.('2025-06-22 20:20:00', 'Binance Convert', 'USDT', '-129'), row.('2025-06-22 20:21:01', 'Binance Convert', 'USDC', '129')]),
  bin.('convert_halves_60s_apart', [row.('2025-06-22 20:20:00', 'Binance Convert', 'USDT', '-129'), row.('2025-06-22 20:21:00', 'Binance Convert', 'USDC', '129')]),
  bin.('convert_credit_before_spend', [row.('2025-06-22 20:20:44', 'Binance Convert', 'ZORK', '5'), row.('2025-06-22 20:20:45', 'Binance Convert', 'FLIB', '-2')]),
  bin.('convert_two_spends_one_credit', [row.('2025-06-22 20:20:00', 'Binance Convert', 'FLIB', '-2'), row.('2025-06-22 20:20:10', 'Binance Convert', 'QUUX', '-3'),
                                         row.('2025-06-22 20:20:05', 'Binance Convert', 'ZORK', '5')]),
  # ---- dust ----
  bin.('dust_without_remark', [row.(t0, 'Small Assets Exchange BNB', 'QUUX', '-3'), row.(t0, 'Small Assets Exchange BNB', 'BLIP', '-0.0076'),
                               row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.0004'), row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.00000919')]),
  bin.('dust_some_remarks_blank', [row.(t0, 'Small Assets Exchange BNB', 'QUUX', '-3', 'QUUX to BNB'), row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.0004', 'QUUX to BNB'),
                                   row.(t0, 'Small Assets Exchange BNB', 'BLIP', '-0.0076'), row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.00000919')]),
  bin.('dust_remark_with_spaces_and_trade', [row.(t0, 'Small Assets Exchange BNB', 'QUUX', '-3', ' QUUX to BNB '), row.(t0, 'Small Assets Exchange BNB', 'BNB', '0.0004', 'QUUX to BNB'),
                                             row.(t0, 'Transaction Spend', 'USDT', '-10'), row.(t0, 'Transaction Buy', 'ZORK', '1')]),
  # ---- signs, zeros, skips ----
  bin.('zero_change_rows', [row.(t0, 'Deposit', 'USDT', '0'), row.(t0, 'Distribution', 'ZORK', '-0'), row.('2024-03-01 10:00:01', 'Airdrop Assets', 'FLIB', '0.000')]),
  bin.('every_operation_positive', Import::BinanceCsv::OPERATIONS.keys.each_with_index.map { |op, i| row.(format('2024-01-01 00:%02d:00', i), op, 'ZORK', "#{i + 1}.5") }),
  bin.('every_operation_negative', Import::BinanceCsv::OPERATIONS.keys.each_with_index.map { |op, i| row.(format('2024-01-01 00:%02d:00', i), op, 'ZORK', "-#{i + 1}.5") }),
  bin.('unknown_and_skips_mixed', [row.(t0, 'Mystery Op', 'ZORK', '1'), row.(t0, 'transfer_in', 'ZORK', '1'), row.(t0, 'Mystery Op', 'ZORK', '2'),
                                   row.(t0, '  Deposit  ', ' USDT ', '5'), row.(t0, '', 'ZORK', '1'), row.(t0, 'deposit', 'ZORK', '1'), row.(t0, 'Other Mystery', 'ZORK', '1')]),
  bin.('only_skips', [row.(t0, 'transfer_out', 'ZORK', '-1'), row.(t0, 'Ledger - Fund Migration', 'ZORK', '1')]),
  # ---- bytes and CSV structure ----
  bin.('double_bom', [row.(t0, 'Deposit', 'USDT', '1')], prefix: BOM + BOM),
  bin.('invalid_utf8_in_remark_and_coin', [row.(t0, 'Deposit', "ZO\xFFRK".b, '1', "bad \xC3\x28 \xE2\x82 \xF0\x9F remark".b), row.(t0, 'Deposit', "\xC0\x80USDT".b, '2')]),
  bin.('invalid_utf8_in_header', [row.(t0, 'Deposit', 'USDT', '1')], prefix: "\xFF".b),
  bin.('crlf', [row.(t0, 'Deposit', 'USDT', '1'), row.('2024-03-01 10:00:01', 'Withdraw', 'USDT', '-1')], sep: "\r\n"),
  bin.('crlf_trailing', [row.(t0, 'Deposit', 'USDT', '1'), ''], sep: "\r\n"),
  bin.('cr_only', [row.(t0, 'Deposit', 'USDT', '1'), row.('2024-03-01 10:00:01', 'Withdraw', 'USDT', '-1')], sep: "\r"),
  ['lf_then_crlf', 'binance', "#{HEADER}\n#{row.(t0, 'Deposit', 'USDT', '1')}\r\n#{row.(t0, 'Deposit', 'USDT', '2')}\n", UTC0],
  bin.('quoted_newline_in_remark', [row.(t0, 'Deposit', 'USDT', '1', "\"line one\nline two\""), row.(t0, 'Deposit', 'ZORK', '2', '"has ""quotes"", and comma"')]),
  bin.('quoted_numbers_and_op', [row.(t0, '"Deposit"', '"USDT"', '"1.25"')]),
  bin.('stray_quote', [row.(t0, 'Deposit', 'US"DT', '1')]),
  bin.('unclosed_quote', [row.(t0, 'Deposit', 'USDT', '1', '"never closed')]),
  bin.('quote_then_text', [row.(t0, 'Deposit', '"USDT"x', '1')]),
  ['empty_file', 'binance', '', UTC0],
  ['bom_only', 'binance', BOM, UTC0],
  bin.('headers_only', []),
  bin.('headers_only_trailing_newline', ['']),
  bin.('blank_lines_between', ['', row.(t0, 'Deposit', 'USDT', '1'), '', '', row.('2024-03-01 10:00:01', 'Deposit', 'USDT', '2'), '']),
  ['duplicate_headers_first_wins', 'binance', "User ID,Time,Account,Operation,Coin,Change,Remark,Coin,Change\n1,#{t0},Spot,Deposit,USDT,1,,ZORK,9\n", UTC0],
  ['reordered_columns', 'binance', "Remark,Change,Coin,Operation,Account,Time,User ID\nr,7,ZORK,Deposit,Spot,#{t0},1\n", UTC0],
  ['header_case_differs', 'binance', "user id,time,account,operation,coin,change,remark\n1,#{t0},Spot,Deposit,USDT,1,\n", UTC0],
  ['short_and_long_rows', 'binance', "#{HEADER}\n1,#{t0},Spot,Deposit,USDT\n1,#{t0},Spot,Deposit,ZORK,3,r,extra,more\n1,#{t0},Spot,Airdrop Assets\n", UTC0],
  ['semicolon_separated', 'binance', "#{HEADER.tr(',', ';')}\n1;#{t0};Spot;Deposit;USDT;1;\n", UTC0],
  # ---- the filename's zone ----
  bin.('zone_utc_plus_3', [row.(t0, 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('zone_utc_minus_5_30', [row.(t0, 'Deposit', 'USDT', '1')], filename: 'Binance-Transaction-History-202601010000(UTC-5:30)-part1-of1.csv'),
  bin.('zone_utc_plus_0', [row.(t0, 'Deposit', 'USDT', '1')], filename: UTC0),
  bin.('zone_utc_plus_5_45', [row.(t0, 'Deposit', 'USDT', '1')], filename: 'export(UTC+5:45).csv'),
  bin.('zone_utc_plus_14_crosses_date', [row.('2024-03-01 05:00:00', 'Deposit', 'USDT', '1')], filename: 'export(UTC+14).csv'),
  bin.('zone_utc_minus_12_crosses_date', [row.('2024-12-31 20:00:00', 'Deposit', 'USDT', '1')], filename: 'export(UTC-12).csv'),
  bin.('zone_utc_plus_99', [row.(t0, 'Deposit', 'USDT', '1')], filename: 'export(UTC+99).csv'),
  bin.('zone_utc_plus_3_99', [row.(t0, 'Deposit', 'USDT', '1')], filename: 'export(UTC+3:99).csv'),
  bin.('zone_utc_minus_0_30', [row.(t0, 'Deposit', 'USDT', '1')], filename: 'export(UTC-0:30).csv'),
  bin.('zone_none', [row.(t0, 'Deposit', 'USDT', '1')], filename: 'Binance-Transaction-History.csv'),
  # Older exports: the column is UTC_Time, the clock is UTC, and the name need carry no zone.
  ['utc_time_header_no_zone_in_name', 'binance', "User_ID,UTC_Time,Account,Operation,Coin,Change,Remark\n1,#{t0},Spot,Deposit,USDT,1,\n", 'renamed.csv'],
  ['utc_time_header_beats_name_zone', 'binance', "User_ID,UTC_Time,Account,Operation,Coin,Change,Remark\n1,#{t0},Spot,Deposit,USDT,1,\n", UTC3],
  ['utc_time_header_with_bom', 'binance', BOM + "User_ID,UTC_Time,Account,Operation,Coin,Change,Remark\n1,#{t0},Spot,Deposit,USDT,1,\n".b, 'renamed.csv'],
  # ---- the time column ----
  bin.('time_fractional_seconds', [row.('2024-03-01 10:00:00.123456789', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_two_digit_year', [row.('24-03-01 10:00:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_slashes', [row.('2024/03/01 10:00:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_date_only', [row.('2024-03-01', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_iso_t', [row.('2024-03-01T10:00:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_own_zone_plus_filename_zone', [row.('2024-03-01 10:00:00+02:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_with_z', [row.('2024-03-01 10:00:00Z', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_padded', [row.('  2024-03-01 10:00:00  ', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_day_month_names', [row.('1 Mar 2024 10:00:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_out_of_range', [row.('2024-02-30 10:00:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_hour_25', [row.('2024-03-01 25:00:00', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_garbage', [row.('yesterday-ish', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_empty', [row.('', 'Deposit', 'USDT', '1')], filename: UTC3),
  bin.('time_garbage_on_skipped_row', [row.('garbage', 'transfer_in', 'USDT', '1'), row.('also garbage', 'Mystery Op', 'USDT', '1')], filename: UTC3),
  # ---- the number column ----
  bin.('change_scientific', [row.(t0, 'Deposit', 'USDT', '1e-5'), row.('2024-03-01 10:00:01', 'Deposit', 'ZORK', '2.5E+3')]),
  bin.('change_padded_and_plus', [row.(t0, 'Deposit', 'USDT', ' 2.5 '), row.('2024-03-01 10:00:01', 'Deposit', 'ZORK', '+3')]),
  bin.('change_garbage', [row.(t0, 'Deposit', 'USDT', 'abc'), row.('2024-03-01 10:00:01', 'Deposit', 'ZORK', '12abc'), row.('2024-03-01 10:00:02', 'Deposit', 'FLIB', '1,5')]),
  bin.('change_empty', [row.(t0, 'Deposit', 'USDT', ''), row.('2024-03-01 10:00:01', 'Deposit', 'ZORK', '7')]),
  bin.('change_many_digits', [row.(t0, 'Deposit', 'USDT', '0.000000000000000000123456789012345678901234567890'),
                              row.('2024-03-01 10:00:01', 'Deposit', 'ZORK', '123456789012345678901234567890.5')])
]

export = lambda do |*rows|
  CsvSafe.generate do |csv|
    csv << AccountTransaction.csv_headers
    rows.each { |r| csv << r }
  end
end
db = ->(name, bytes, filename: 'deltabadger-transactions.csv') { [name, 'deltabadger', bytes, filename] }
dheader = AccountTransaction.csv_headers.join(',')

deltabadger = [
  db.('integration_deposit', "#{dheader}\n2021-07-01T14:07:04Z,deposit,USDT,150.2,,,,,binance,tx-1,,", filename: 'whatever.csv'),
  db.('export_round_trip', export.(
    ['2024-01-02T03:04:05Z', 'buy', 'ZORK', BigDecimal('1.5'), 'USD', BigDecimal('30'), 'USD', BigDecimal('0.03'), 'kraken', 'o-1', nil, 'plain note'],
    ['2024-01-01T00:00:00Z', 'swap_out', 'FLIB', BigDecimal('2'), nil, nil, nil, nil, 'binance', nil, 'g-1', nil],
    ['2024-01-01T00:00:00Z', 'swap_in', 'ZORK', BigDecimal('4'), nil, nil, 'BNB', BigDecimal('0.001'), 'binance', nil, 'g-1', nil],
    ['2024-01-03T00:00:00Z', 'withdrawal', 'USDT', BigDecimal('10'), nil, nil, nil, nil, 'alpaca', 'w-1', nil, 'Withdraw fee is included'],
    ['2024-01-04T00:00:00Z', 'other_income', 'ZORK', BigDecimal('1'), nil, nil, nil, nil, 'binance', nil, nil, '=SUM(A1:A2)'],
    ['2024-01-05T00:00:00Z', 'lost', 'ZORK', BigDecimal('1'), nil, nil, nil, nil, 'binance', nil, nil, '-fee note'],
    ['2024-01-06T00:00:00Z', 'deposit', 'ZORK', BigDecimal('1'), nil, nil, nil, nil, 'binance', nil, nil, ' @mention'],
    ['2024-01-07T00:00:00Z', 'deposit', 'ZORK', BigDecimal('1'), nil, nil, nil, nil, 'binance', nil, nil, "'apostrophe kept"],
    ['2024-01-08T00:00:00Z', 'deposit', 'ZORK', BigDecimal('1'), nil, nil, nil, nil, 'binance', nil, nil, "+1\nsecond line"]
  )),
  db.('invalid_and_blank_types', "#{dheader}\n2024-01-01T00:00:00Z,bogus,ZORK,1,,,,,binance,,,\n2024-01-02T00:00:00Z,,ZORK,1,,,,,binance,,,\n" \
                                 "2024-01-03T00:00:00Z,Deposit,ZORK,1,,,,,binance,,,\n2024-01-04T00:00:00Z,bogus,ZORK,1,,,,,binance,,,\n" \
                                 "2024-01-05T00:00:00Z, deposit ,ZORK,1,,,,,binance,,,\n"),
  db.('only_invalid_types', "#{dheader}\n2024-01-01T00:00:00Z,bogus,ZORK,1,,,,,binance,,,\n"),
  db.('only_blank_types', "#{dheader}\n2024-01-01T00:00:00Z,,ZORK,1,,,,,binance,,,\n"),
  db.('unconnected_venue_and_blank_venue', "#{dheader}\n2024-01-01T00:00:00Z,deposit,ZORK,1,,,,,nowhere,,,\n2024-01-02T00:00:00Z,deposit,ZORK,1,,,,, ,,,\n"),
  db.('fields_not_stripped', "#{dheader}\n2024-01-01T00:00:00Z,buy, ZORK ,1, USD ,2, ,,binance, tx ,,  \n"),
  db.('date_without_zone', "#{dheader}\n2024-01-01 12:00:00,deposit,ZORK,1,,,,,binance,,,\n"),
  db.('date_with_offset', "#{dheader}\n2024-01-01T12:00:00+02:00,deposit,ZORK,1,,,,,binance,,,\n"),
  db.('date_fractional', "#{dheader}\n2024-01-01T12:00:00.5Z,deposit,ZORK,1,,,,,binance,,,\n"),
  db.('date_garbage', "#{dheader}\n2024-01-01T00:00:00Z,deposit,ZORK,1,,,,,binance,,,\nnot a date,deposit,ZORK,1,,,,,binance,,,\n"),
  db.('date_empty', "#{dheader}\n,deposit,ZORK,1,,,,,binance,,,\n"),
  db.('amounts_lenient', "#{dheader}\n2024-01-01T00:00:00Z,buy,ZORK,abc,USD,12abc,USD,1e-3,binance,,,\n2024-01-02T00:00:00Z,deposit,ZORK,,,,,,binance,,,\n"),
  db.('headers_only', "#{dheader}\n"),
  db.('bom_crlf', BOM + "#{dheader}\r\n2024-01-01T00:00:00Z,deposit,ZORK,1,,,,,binance,,,\r\n".b),
  db.('stray_quote', "#{dheader}\n2024-01-01T00:00:00Z,deposit,ZO\"RK,1,,,,,binance,,,\n"),
  db.('sorted_stably_by_date', "#{dheader}\n2024-01-02T00:00:00Z,deposit,B,1,,,,,binance,,,\n2024-01-01T00:00:00Z,deposit,A,1,,,,,binance,,,\n" \
                               "2024-01-02T00:00:00Z,deposit,C,1,,,,,binance,,,\n2024-01-01T00:00:00Z,deposit,D,1,,,,,binance,,,\n"),
  db.('binance_file_as_deltabadger', ([HEADER, row.(t0, 'Deposit', 'USDT', '1')]).join("\n")),
  ['deltabadger_file_as_binance', 'binance', "#{dheader}\n2024-01-01T00:00:00Z,deposit,ZORK,1,,,,,binance,,,\n", UTC0]
]

vectors = {}
vectors['tz'] = ENV['TZ']
vectors['now'] = NOW.iso8601(6)
vectors['binance'] = binance.map { |c| record.(*c) }
vectors['deltabadger'] = deltabadger.map { |c| record.(*c) }
%w[binance deltabadger].each do |group|
  names = vectors[group].map { |c| c['name'] }
  raise "duplicate #{group} case names: #{names.tally.select { |_, n| n > 1 }.keys}" if names.uniq.size != names.size
end

vectors['offset_from'] = [
  UTC0, UTC3, 'Binance-Transaction-History-202601010000(UTC-5:30)-part1-of1.csv', 'x(UTC+5:45).csv', 'x(UTC+14).csv', 'x(UTC-12).csv',
  'x(UTC+03).csv', 'x(UTC+3:00).csv', 'x(UTC-0:30).csv', 'x(UTC-0).csv', 'x(UTC+99).csv', 'x(UTC+100).csv', 'x(UTC+3:5).csv', 'x(UTC+3:30:00).csv',
  'x(UTC3).csv', 'x(utc+3).csv', 'x(UTC +3).csv', 'x(UTC+3 ).csv', 'x UTC+3.csv', 'x(UTC+3)(UTC-4).csv', 'x(UTC+3:99).csv', 'x(UTC+١).csv',
  'renamed.csv', '', nil
].map { |name| [name, Import::BinanceCsv.offset_from(name)] }

vectors['as_text'] = [
  '', BOM, BOM + BOM, "a#{BOM}b", "\xFF", "a\xC3", "\xE2\x82", "\xE2\x82x", "\xC0\x80", "\xED\xA0\x80", "\xF4\x90\x80\x80", "\xF0\x9F\x98",
  "\xF0\x9F\x98\x80", "\x80\x80\x80", "ok\xFE\xFFok", "\xEF\xBB", "\xEF\xBB\xBF\xFF", "\r\n\r", "caf\xC3\xA9"
].map { |bytes| [Base64.strict_encode64(bytes.b), Import::Run.as_text(bytes.b)] }

vectors['to_d'] = [
  '0', '1', '-1', '1.5', '-0.000001', '.5', '5.', '-.5', '+1', '+.5', '00012.50', '1e-5', '1E+3', '1.2e3', '-2.5E-2', '1e', 'e5', '1e+', '1e400', '1e-400',
  ' 1.5', '1.5 ', "\t1.5\n", '1_000', '1__0', '_1', '1,5', '1,000.5', '1.2.3', '--1', '-+1', 'abc', '12abc', 'abc12', '', ' ', '0x1A', '0b11', '1/2',
  'Infinity', '-Infinity', 'NaN', 'inf', '١٢', '１２', '12 34', '0.1e1.5', '123456789012345678901234567890.123456789012345678901234567890'
].map { |s| [s, s.to_d.to_s('F')] }

vectors['operations'] = Import::BinanceCsv::OPERATIONS.transform_values { |kind| kind&.to_s }
vectors['cash'] = Import::BinanceCsv::CASH.source
vectors['convert_window'] = Import::BinanceCsv::CONVERT_WINDOW
vectors['csv_headers'] = AccountTransaction.csv_headers
vectors['entry_types'] = AccountTransaction.entry_types

vectors['ported_sources'] = %w[
  app/models/import/binance_csv.rb app/models/import/deltabadger_csv.rb app/models/import/run.rb
  app/controllers/tracker/imports_controller.rb app/models/account_transaction.rb app/models/concerns/csv_safe.rb
].to_h { |file| [file, Digest::SHA256.file(Rails.root.join(file)).hexdigest] }
lock = File.read(Rails.root.join('Gemfile.lock'))
vectors['gems'] = %w[csv bigdecimal].to_h { |gem| [gem, lock[/^    #{gem} \((.+)\)$/, 1]] }

File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}: #{vectors['binance'].size} binance, #{vectors['deltabadger'].size} deltabadger, " \
     "#{vectors['offset_from'].size} offset_from, #{vectors['as_text'].size} as_text, #{vectors['to_d'].size} to_d"
