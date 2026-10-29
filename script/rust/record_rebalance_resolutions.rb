# Rails' actual resolution method, with only persistence replaced by an in-memory merge.
# Rows are real Transaction objects, so enum predicates and every branch remain Rails'.
require 'json'
require 'digest'
sources = %w[script/rust/record_rebalance_resolutions.rb app/controllers/bots/rebalance_resolutions_controller.rb
             app/models/bot/rebalanceable.rb app/models/bot.rb app/models/transaction.rb app/views/bots/composition/_metrics.html.erb
             config/locales/bot.en.yml script/rust/pages.rb script/rust/page_figures.rb]
cases = [nil, '0', '40'].product([nil, 'closed', 'cancelled', 'abandoned'], [nil, 'closed', 'cancelled', 'abandoned']).map do |owed, sell, buy|
  rows = [[11, sell], [12, buy]].filter_map { |id, status| Transaction.new(id:, external_status: status) if status }
  pending = { 'phase' => 'ambiguous', 'sell_transaction_id' => (11 if sell), 'buy_transaction_id' => (12 if buy),
              'remaining_quote_amount' => owed, 'buy_attempted' => true }
  bot = Bots::DcaMultiAsset.new(transient_data: { 'rebalance_pending' => pending, 'unrelated' => 17, 'old_null' => nil })
  relation = Object.new
  relation.define_singleton_method(:find_by) { |id:| rows.find { |row| row.id == id } }
  bot.define_singleton_method(:transactions) { relation }
  bot.define_singleton_method(:merge_transient_data!) { |values| self.transient_data = transient_data.merge(values).compact }
  bot.resolve_rebalance_halt!
  { pending:, sell:, buy:, expected: bot.transient_data }
end
File.write(ARGV.fetch(0), JSON.pretty_generate({ sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases: }) + "\n")
puts "recorded #{cases.size} rebalance resolutions"
