# Records Bot::SignalJob rows as Solid Queue holds them, for rust/tests/handover.rs (plan 8d T2, r2 §R1): the hook's
# enqueue (HooksController#enqueue: claim_trigger!, then perform_later with the claim's last_triggered_at) on a working
# signal bot, in each state the takeover meets: ready, blocked behind the venue's semaphore, scheduled (an unresolved
# placement's retry_job), finished, failed, and one whose rule or bot was removed after the enqueue. Each job keeps its
# solid_queue_jobs row, its execution row and the triggered_at Rails deserializes from its arguments.
#   bin/rails runner script/rust/record_signal_jobs.rb rust/tests/fixtures/signal_jobs.json
# Nothing leaves the machine. Re-run (script/rust/repin) when a file under "sources" changes.
require 'json'
require 'digest'
require 'tmpdir'
require 'active_support/testing/time_helpers'
require Rails.root.join('script/rust/schema_template')
extend ActiveSupport::Testing::TimeHelpers

dir = Dir.mktmpdir('signal-jobs')
SchemaTemplate.install(dir)
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
SolidQueue::Record.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production_queue.sqlite3'))
ActiveJob::Base.queue_adapter = :solid_queue
ActiveJob::Base.logger = Logger.new(File::NULL)

at = Time.utc(2026, 9, 30, 11, 58, 0, 123_456)
cases = {}
travel_to(at, with_usec: true)
begin
  user = User.new(name: 'Owner', email: 'owner@example.com', password: 'synthetic-password', confirmed_at: at)
  user.save!(validate: false)
  exchange = Exchanges::Kraken.create!(name: 'Kraken')
  btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
  eur = Asset.create!(external_id: 'eur', symbol: 'EUR', name: 'Euro', category: 'Fiat')
  [btc, eur].each { |asset| ExchangeAsset.create!(exchange:, asset:, available: true) }
  Ticker.create!(exchange:, ticker: 'XBTEUR', base: 'BTC', quote: 'EUR', base_asset: btc, quote_asset: eur,
                 base_decimals: 8, quote_decimals: 5, price_decimals: 1, minimum_base_size: '0.00005', minimum_quote_size: '0.5')
  bots = Array.new(2) do
    bot = user.bots.new(type: 'Bots::Signal', exchange:, settings: { 'base_asset_id' => btc.id, 'quote_asset_id' => eur.id })
    bot.save!
    bot.update_columns(status: Bot.statuses.fetch('scheduled'), started_at: at - 1.day)
    bot
  end
  rule = ->(bot) { BotSignal.create!(bot:, direction: 'buy', amount_type: 'fixed', amount: '100') }
  # HooksController#enqueue, with a fixed job id so that a re-record on unchanged sources writes the same bytes.
  enqueue = lambda do |name, bot, signal, wait: nil|
    travel_back
    travel_to(at + (cases.size + 1).seconds, with_usec: true)
    raise "#{name}: claim" unless signal.claim_trigger!

    job = Bot::SignalJob.new(bot.reload, signal, signal.last_triggered_at)
    job.job_id = "00000000-0000-4000-8000-#{format('%012d', cases.size + 1)}"
    job.scheduled_at = Time.current + wait if wait
    raise "#{name}: enqueue" unless job.enqueue

    cases[name] = { 'job_id' => job.job_id, 'bot_id' => bot.id, 'signal_id' => signal.id }
  end
  enqueue.('ready', bots[0], rule.(bots[0]))
  enqueue.('blocked', bots[0], rule.(bots[0]))
  # Bot::SignalJob#wait_for_resolution: retry_job(wait: UNRESOLVED_RETRY_WAIT).
  enqueue.('scheduled', bots[0], rule.(bots[0]), wait: Bot::SignalJob::UNRESOLVED_RETRY_WAIT)
  enqueue.('finished', bots[0], rule.(bots[0]), wait: 1.hour)
  enqueue.('failed', bots[0], rule.(bots[0]), wait: 1.hour)
  removed = rule.(bots[0])
  enqueue.('rule_removed', bots[0], removed, wait: 1.hour)
  removed.delete
  enqueue.('bot_removed', bots[1], rule.(bots[1]), wait: 1.hour)
  BotSignal.where(bot_id: bots[1].id).delete_all
  Bot.where(id: bots[1].id).delete_all
  finished = SolidQueue::Job.find_by!(active_job_id: cases['finished']['job_id'])
  finished.scheduled_execution.destroy!
  finished.update_columns(finished_at: Time.current)
  SolidQueue::Job.find_by!(active_job_id: cases['failed']['job_id']).scheduled_execution.tap do |execution|
    SolidQueue::FailedExecution.create!(job_id: execution.job_id, error: { exception_class: 'RuntimeError', message: 'boom', backtrace: [] }.to_json)
    execution.delete
  end
ensure
  travel_back
end

row = ->(record) { record.attributes_before_type_cast.except('id').transform_values { |v| v.is_a?(Time) ? v.utc.iso8601(6) : v } }
cases.each do |name, c|
  job = SolidQueue::Job.find_by!(active_job_id: c['job_id'])
  execution = %w[ready blocked scheduled claimed failed].find { |kind| SolidQueue.const_get("#{kind.capitalize}Execution").exists?(job_id: job.id) }
  c['state'] = job.finished_at ? 'finished' : execution
  c['row'] = row.(job)
  c['execution'] = (row.(SolidQueue.const_get("#{execution.capitalize}Execution").find_by!(job_id: job.id)).except('job_id') if execution)
  # What Bot::SignalJob#perform receives, or that it is discarded (discard_on ActiveJob::DeserializationError).
  begin
    c['triggered_at'] = ActiveJob::Base.deserialize(JSON.parse(job.attributes_before_type_cast['arguments'])).tap { |j| j.send(:deserialize_arguments_if_needed) }.arguments[2].utc.iso8601(9)
  rescue ActiveJob::DeserializationError
    c['triggered_at'] = nil
  end
end
sources = %w[script/rust/record_signal_jobs.rb app/jobs/bot/signal_job.rb app/jobs/bot_job.rb app/controllers/hooks_controller.rb
             app/models/bot_signal.rb db/queue_schema.rb]
File.write(ARGV.fetch(0), JSON.pretty_generate({ 'sources' => sources.to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] },
                                                'jobs' => cases }) + "\n")
FileUtils.rm_rf(dir)
