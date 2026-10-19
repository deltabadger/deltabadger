# Records what ApiKeyValidatorJob does to a pending Kraken, Binance or KuCoin key, for the Rust validator job
# (rust/tests/settings.rs validator_job_matches_rails_on_every_checked_venue).
#   bin/rails runner script/rust/record_validator_effects.rb rust/tests/fixtures/validator_effects.json
# Each case stores one key as POST /api/api_keys leaves it (pending_validation), scripts the venue's answer to its key
# check (WebMock; nothing leaves the machine), performs the job and records the request, the key's status after it,
# the jobs it enqueued and the warnings it logged. Re-run (script/rust/repin) when a file under "sources" changes.
require 'json'
require 'digest'
require 'webmock'

out = {
  'sources' => %w[app/jobs/api_key_validator_job.rb app/services/api_key_validator.rb app/models/api_key.rb app/models/exchange.rb
                  app/models/exchanges/kraken.rb app/models/exchanges/binance.rb app/models/exchanges/kucoin.rb
                  script/rust/record_validator_effects.rb]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'honeymaker' => Honeymaker::VERSION
}

Rails.configuration.dry_run = false # the test environment answers every key check true without asking
ActiveJob::Base.queue_adapter = :test
WebMock.enable!
WebMock.disable_net_connect!
logged = []
Rails.logger = ActiveSupport::Logger.new(StringIO.new).tap do |logger|
  logger.formatter = ->(severity, _, _, message) { logged << message.to_s if severity == 'WARN' && message.to_s.include?('API key validation') }
end

json = ->(status, body) { { status:, body: body.to_json, headers: { 'Content-Type' => 'application/json' } } }
binance_flags = { 'ipRestrict' => true, 'enableFixApiTrade' => false, 'enableFixReadOnly' => false, 'enableFutures' => false,
                  'enableInternalTransfer' => false, 'enableMargin' => false, 'enablePortfolioMarginTrading' => false, 'enableReading' => true,
                  'enableSpotAndMarginTrading' => true, 'enableVanillaOptions' => false, 'enableWithdrawals' => false, 'permitsUniversalTransfer' => false }
venues = {
  'Kraken' => lambda do |key_type, answer|
    required = Exchanges::Kraken::KEY_PERMISSIONS[key_type][:required]
    forbidden = Exchanges::Kraken::KEY_PERMISSIONS[key_type][:forbidden]
    body = { 'correct' => { 'error' => [], 'result' => { 'permissions' => required } },
             'rejected' => { 'error' => ['EAPI:Invalid key'] },
             'inconclusive' => { 'error' => ['EService:Unavailable'] },
             'permission' => { 'error' => [], 'result' => { 'permissions' => required.drop(1) + forbidden.take(1) } } }.fetch(answer)
    [:post, 'https://api.kraken.com/0/private/GetApiKeyInfo', json.(200, body)]
  end,
  'Binance' => lambda do |key_type, answer|
    flags = key_type == :withdrawal ? binance_flags.merge('enableWithdrawals' => true, 'enableSpotAndMarginTrading' => false) : binance_flags
    reply = { 'correct' => json.(200, flags), 'rejected' => json.(401, { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' }),
              'inconclusive' => { status: 500, body: '' }, 'permission' => json.(200, flags.merge('ipRestrict' => false)) }.fetch(answer)
    [:get, %r{\Ahttps://api\.binance\.com/sapi/v1/account/apiRestrictions}, reply]
  end,
  'Kucoin' => lambda do |key_type, answer|
    probe = key_type == :trading
    reply = { 'correct' => json.(200, { 'code' => '200000', 'data' => probe ? { 'cancelledOrderIds' => [] } : [] }),
              'rejected' => json.(401, { 'code' => '400003', 'msg' => 'KC-API-KEY not exists' }),
              'inconclusive' => json.(429, { 'code' => '429000', 'msg' => 'Too Many Requests' }),
              # Balances #get_balances cannot walk: it raises, and the job is discarded.
              'data_false' => json.(200, { 'code' => '200000', 'data' => false }),
              'data_without_currency' => json.(200, { 'code' => '200000', 'data' => [{ 'available' => '1' }] }) }.fetch(answer)
    probe ? [:delete, "https://api.kucoin.com/api/v1/orders/#{'0' * 24}", reply] : [:get, %r{\Ahttps://api\.kucoin\.com/api/v1/accounts}, reply]
  end
}

seen = []
WebMock.after_request { |request, _| seen << { 'method' => request.method.to_s.upcase, 'host' => request.uri.host, 'path' => request.uri.path } }
user = User.create!(name: 'Owner', email: 'owner@example.com', password: 'Correct-horse-9', confirmed_at: Time.current)
cases = []
venues.each do |klass, answers|
  exchange = "Exchanges::#{klass}".constantize.create!(name: klass == 'Kucoin' ? 'KuCoin' : klass, maker_fee: 0, taker_fee: 0)
  %i[trading read_only withdrawal].each do |key_type|
    names = %w[correct rejected inconclusive]
    names << 'permission' unless klass == 'Kucoin'
    names += %w[data_false data_without_currency] if klass == 'Kucoin' && key_type == :read_only
    names.each do |answer|
      method, url, reply = answers.call(key_type, answer)
      WebMock.reset!
      seen.clear
      WebMock.stub_request(method, url).to_return(reply)
      ApiKey.where(user:).delete_all
      api_key = ApiKey.create!(user:, exchange:, key_type:, status: :pending_validation, key: 'test-key', secret: Base64.strict_encode64('test-secret'),
                               passphrase: klass == 'Kucoin' ? 'test-passphrase' : nil)
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      logged.clear
      ApiKeyValidatorJob.perform_now(api_key.id)
      cases << { 'venue' => exchange.type, 'name' => exchange.name, 'key_type' => key_type.to_s, 'answer' => answer,
                 'reply' => { 'status' => reply[:status], 'body' => reply[:body] }, 'requests' => seen.dup,
                 'status' => api_key.reload.status, 'jobs' => ActiveJob::Base.queue_adapter.enqueued_jobs.map { |job| job[:job].name },
                 'logged' => logged.map { |line| line.sub(/(failed):.*\z/m, '\\1').strip } }
    end
  end
end
out['cases'] = cases

File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{ARGV[0]}: #{cases.size} cases"
