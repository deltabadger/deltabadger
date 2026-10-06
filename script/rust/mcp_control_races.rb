# Real Rails BotApi and ActionJob; hooks control scheduling only, never outcomes.
source = Rails.root.join('script/rust/mcp_control.rb').read.split("command, root = ARGV\ncase command\n").first
# The pinned, local recorder source supplies shared fixtures; no external code is evaluated.
eval(source, TOPLEVEL_BINDING, Rails.root.join('script/rust/mcp_control.rb').to_s) # rubocop:disable Security/Eval
source = Rails.root.join('script/rust/decisions.rb').read.split("command, root = ARGV\n").first
# The pinned, local recorder source supplies shared fixtures; no external code is evaluated.
eval(source, TOPLEVEL_BINDING, Rails.root.join('script/rust/decisions.rb').to_s) # rubocop:disable Security/Eval
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedAlpaca::Adapter)
Rails.configuration.dry_run = false # All HTTP is fenced by ScriptedAlpaca and Net::HTTP above.
root = File.expand_path(ARGV.fetch(0))
raise 'race root must be empty' if Dir.exist?(root) && !Dir.empty?(root)

FileUtils.mkdir_p(root)
template = Pages.template(root)
McpControl.seed(template)
# Bind the queue too: cancellations execute real SolidQueue queries against fixture data.
Pages.singleton_class.define_method(:connect) do |dir|
  ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
  SolidQueue::Record.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production_queue.sqlite3'))
end
McpParity.singleton_class.define_method(:snapshot) { McpControl.snapshot }
loaded = Queue.new
release = Queue.new
BotApi::Bots::UpdateSettings.prepend(Module.new do
  define_method(:build_updates) do |bot|
    if Thread.current[:pause_m4_label]
      loaded << bot.quote_amount
      release.pop
    end
    super(bot)
  end
end)
dir = File.join(root, 'settings_lost_update')
FileUtils.mkdir_p(dir)
%w[production.sqlite3 production_queue.sqlite3 secret_key_base].each { |f| FileUtils.cp(File.join(template, f), File.join(dir, f)) }
Pages.connect(dir)
McpParity.travel_to(Time.iso8601(Pages::AT), with_usec: true) do
  Bot.find(1).update_columns(settings: Bot.find(1).settings.except('limit_order_pcnt_distance').merge('quote_amount' => 50))
  thread = Thread.new do
    ActiveRecord::Base.connection_pool.with_connection do
      Thread.current[:pause_m4_label] = true
      BotApi::Bots::UpdateSettings.call(user: User.find(1), bot_id: 1, label: 'Concurrent rename')
    end
  end
  read = loaded.pop
  amount = BotApi::Bots::UpdateSettings.call(user: User.find(1), bot_id: 1, quote_amount: 20)
  committed = Bot.find(1).quote_amount
  release << true
  label = thread.value
  final = Bot.find(1).quote_amount
  unless amount.success? && label.success? && read == 50 && committed == 20 && final == 50
    raise "lost-update schedule changed: #{[read, committed, final, amount.to_h,
                                            label.to_h].inspect}"
  end

  File.write(File.join(dir, 'race.json'),
             JSON.pretty_generate({ read:, committed:, final:, label: Bot.find(1).label, amount: amount.to_h, rename: label.to_h,
                                    rows: McpControl.snapshot }))
end
# Every tool races a real tick, paused below the real client at its first price response.
entered = Queue.new
resume = Queue.new
ScriptedAlpaca.singleton_class.prepend(Module.new do
  define_method(:reply) do |env|
    if Thread.current[:pause_m4_price] && env.url.path == '/v1beta3/crypto/us/latest/quotes'
      Thread.current[:pause_m4_price] = false
      entered << true
      resume.pop
    end
    super(env)
  end
end)
McpControl::NAMES.each do |name|
  dir = File.join(root, name)
  FileUtils.mkdir_p(dir)
  %w[production.sqlite3 production_queue.sqlite3 secret_key_base].each { |f| FileUtils.cp(File.join(template, f), File.join(dir, f)) }
  Pages.connect(dir)
  McpParity.configure
  ActiveJob::Base.queue_adapter.enqueued_jobs.clear
  Rails.cache.clear
  ScriptedAlpaca.http = Decisions.basket_http.transform_values(&:dup)
  ScriptedAlpaca.sent = []
  McpParity.travel_to(Time.iso8601(Pages::AT), with_usec: true) do
    bot = Bot.find(1)
    bot.update_columns(status: 1, started_at: Time.iso8601('2026-09-09T12:00:00Z'), settings_changed_at: nil)
    before = McpControl.snapshot
    ActiveRecord::Base.connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    FileUtils.cp(File.join(dir, 'production.sqlite3'), File.join(dir, 'before.sqlite3'))
    File.write(File.join(dir, 'tool.json'), JSON.generate(McpControl.tool(name, name == 'update_bot_settings' ? { quote_amount: 37.25 } : {})))
    thread = Thread.new do
      ActiveRecord::Base.connection_pool.with_connection do
        Thread.current[:pause_m4_price] = true
        Rails.application.executor.wrap { Bot::ActionJob.perform_now(Bot.find(1)) }
      end
    end
    # Bound every barrier; a fixture failure may not leave a thread waiting forever.
    Timeout.timeout(20) { entered.pop }
    browser = ActionDispatch::Integration::Session.new(Rails.application)
    browser.host! 'localhost:3000'
    sid = nil
    response = nil
    before_tool = nil
    (McpParity.ready + [McpControl.tool(name, name == 'update_bot_settings' ? { quote_amount: 37.25 } : {})]).each do |step|
      before_tool = McpControl.snapshot if JSON.parse(step['body'])['method'] == 'tools/call'
      hs = step['headers'].transform_values { |v| v == '$session' ? sid : v }
      browser.process(:post, step['path'], params: step['body'], headers: hs)
      sid = browser.response.headers['Mcp-Session-Id'] || sid
      response = { status: browser.response.status, body: browser.response.body }
    end
    at_tool = McpControl.snapshot
    resume << true
    Timeout.timeout(20) { thread.join }
    thread.value # Re-raise a job/harness exception, never swallow it.
    sent = ScriptedAlpaca.sent
    final = McpControl.snapshot
    File.write(File.join(dir, 'diagnostic.json'), JSON.pretty_generate({ response:, sent:, at_tool:, final: }))
    raise "scripted tick placements changed: #{sent.inspect}" unless sent.size == 2
    raise 'wrong total contribution' unless sent.sum { |order| order.fetch('notional').to_d } == 500

    File.write(File.join(dir, 'race.json'), JSON.pretty_generate({ response:, before:, before_tool:, at_tool:, final:, sent: }))
  ensure
    resume << true if thread&.alive?
    thread&.join(20)
  end
end
puts 'Rails race outcomes recorded: settings plus six tools'
# Fresh start: the actual MCP service queues immediately, then the real job buys once.
dir = File.join(root, 'fresh_start')
FileUtils.mkdir_p(dir)
%w[production.sqlite3 production_queue.sqlite3 secret_key_base].each { |file| FileUtils.cp(File.join(template, file), File.join(dir, file)) }
Pages.connect(dir)
McpParity.configure
ActiveJob::Base.queue_adapter.enqueued_jobs.clear
Rails.cache.clear
ScriptedAlpaca.http = Decisions.basket_http.transform_values(&:dup)
ScriptedAlpaca.sent = []
fresh_tool = nil
fresh_job = nil
McpParity.travel_to(Time.iso8601(Pages::AT), with_usec: true) do
  Bot.find(1).update_columns(status: 0, started_at: nil)
  ActiveRecord::Base.connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')
  FileUtils.cp(File.join(dir, 'production.sqlite3'), File.join(dir, 'before.sqlite3'))
  steps = McpParity.ready + [McpControl.tool('start_bot')]
  File.write(File.join(dir, 'steps.json'), JSON.generate(steps))
  browser = ActionDispatch::Integration::Session.new(Rails.application)
  browser.host! 'localhost:3000'
  sid = nil
  responses = steps.map do |step|
    headers = step['headers'].transform_values { |value| value == '$session' ? sid : value }
    browser.process(:post, step['path'], params: step['body'], headers: headers)
    response = browser.response
    sid = response.headers['Mcp-Session-Id'] || sid
    { status: response.status, body: response.body, headers: McpParity::HEADERS.to_h { |header| [header, response.headers[header]] }.compact }
  end
  fresh_tool = { responses:, session: sid, rows: McpControl.snapshot }
  fresh_job = ActiveJob::Base.queue_adapter.enqueued_jobs.find { |queued| queued[:job] == Bot::ActionJob }
  raise 'fresh start did not queue now' unless fresh_job && fresh_job[:at].nil?
end
# A queued job begins after the service, on the strict checkpoint boundary.
McpParity.travel_to(Time.iso8601(Pages::AT) + Rational(1, 1_000_000), with_usec: true) do
  Rails.application.executor.wrap { Bot::ActionJob.perform_now(Bot.find(1)) }
  sent = ScriptedAlpaca.sent
  contribution = sent.sum { |order| order.fetch('notional').to_d }
  raise "fresh start contribution changed: #{sent.inspect}" unless sent.size == 2 && contribution == 20

  File.write(File.join(dir, 'fresh.json'), JSON.pretty_generate({ tool: fresh_tool, job_at: fresh_job[:at], sent:, rows: McpControl.snapshot }))
end
