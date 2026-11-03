# Records AppUpdate and Clients::Github for rust/tests/app_update.rs (jobs::app_update): what refresh! returns and leaves in
# the cache for scripted GitHub answers, the request it sends, available? per (current, latest), enabled? per environment,
# and Installation.platform per marker.
#   bin/rails runner script/rust/record_app_update.rb rust/tests/fixtures/app_update_vectors.json
# Re-record when a pinned source moves (script/rust/repin).
require 'json'
require 'digest'
require 'webmock'

out = ARGV.fetch(0)
pinned = %w[app/models/app_update.rb app/models/clients/github.rb app/models/installation.rb app/jobs/app_update/check_job.rb
            config/application.rb]
sources = pinned.to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }

include WebMock::API
WebMock.enable!
WebMock.disable_net_connect!
Rails.cache = ActiveSupport::Cache::MemoryStore.new
url = 'https://api.github.com/repos/deltabadger/deltabadger/releases/latest'
json = { 'Content-Type' => 'application/json' }
requests = []
WebMock.after_request { |req, _| requests << { 'method' => req.method.to_s.upcase, 'url' => req.uri.to_s, 'headers' => req.headers.slice('Accept', 'User-Agent') } }

with_env = lambda do |env, &block|
  saved = env.to_h { |k, _| [k, ENV.fetch(k, nil)] }
  env.each { |k, v| v.nil? ? ENV.delete(k) : ENV.store(k, v) }
  block.call
ensure
  saved.each { |k, v| v.nil? ? ENV.delete(k) : ENV.store(k, v) }
end

# Each answer in order, against one cache: a success first, then answers that must keep it.
answers = [
  ['success', { status: 200, body: { 'tag_name' => 'v3.0.1', 'html_url' => 'https://evil.invalid/' }.to_json, headers: json }],
  ['server_error', { status: 500, body: '{"message":"oops"}', headers: json }],
  ['timeout', :timeout],
  ['prerelease_tag', { status: 200, body: { 'tag_name' => 'v3.1.0-beta.1' }.to_json, headers: json }],
  ['nightly_tag', { status: 200, body: { 'tag_name' => 'nightly' }.to_json, headers: json }],
  ['array_body', { status: 200, body: '[]', headers: json }],
  ['not_json', { status: 200, body: 'hello', headers: { 'Content-Type' => 'text/plain' } }],
  ['redirect', { status: 302, body: '', headers: { 'Location' => 'https://evil.invalid/' } }],
  ['redirect_with_release', { status: 302, body: { 'tag_name' => 'v3.0.3' }.to_json, headers: json.merge('Location' => 'https://evil.invalid/') }],
  ['not_found_with_release', { status: 404, body: { 'tag_name' => 'v3.0.4' }.to_json, headers: json }],
  ['missing_tag', { status: 200, body: '{}', headers: json }],
  ['bare_tag', { status: 200, body: { 'tag_name' => '3.0.2' }.to_json, headers: json }],
  ['four_part_tag', { status: 200, body: { 'tag_name' => 'v3.0.2.1' }.to_json, headers: json }]
]
refresh = with_env.({ 'DELTABADGER_PLATFORM' => 'docker', 'DELTABADGER_UPDATE_CHECK' => nil }) do
  answers.map do |name, answer|
    WebMock.reset!
    requests.clear
    stub = stub_request(:get, url)
    answer == :timeout ? stub.to_timeout : stub.to_return(answer)
    returned = AppUpdate.refresh!
    { 'case' => name, 'returned' => returned&.transform_keys(&:to_s), 'cached' => AppUpdate.latest&.transform_keys(&:to_s),
      'requests' => requests.dup }
  end
end

# Desktop and opted-out installs: refresh! sends nothing.
silent = [{ 'DELTABADGER_PLATFORM' => 'desktop', 'DELTABADGER_UPDATE_CHECK' => nil },
          { 'DELTABADGER_PLATFORM' => nil, 'DELTABADGER_UPDATE_CHECK' => 'false' }].map do |env|
  WebMock.reset!
  requests.clear
  Rails.cache.clear
  stub_request(:get, url).to_return(status: 200, body: { 'tag_name' => 'v3.0.1' }.to_json, headers: json)
  returned = with_env.(env) { AppUpdate.refresh! }
  { 'env' => env, 'returned' => returned, 'requests' => requests.dup }
end

# available? per (current, latest).
original = AppUpdate.method(:current_version)
pairs = [%w[2.70.21 3.0.0], %w[3.0.0-beta.2 3.0.0], %w[3.0.0 3.0.0], %w[3.0.1 3.0.0], %w[3.0 3.0.0], %w[3 3.0.1],
         %w[3.0.0.1 3.0.0], %w[3.0.0 3.0.0.1], %w[3.0.0-rc1 3.0.0], %w[3.0.0-beta.2 2.99.99], %w[3.0.0-beta.10 3.0.0],
         %w[10.0.0 9.9.9], %w[3.0.0+build 3.0.0], %w[garbage 3.0.0], ['', '3.0.0'], %w[v3.0.0 3.0.1], %w[3.0.0 99.0.0],
         %w[3.0.0 0.0.1], ['3.0.0 ', '3.0.1'], %w[03.0.0 3.0.1], %w[3.0.0-1 3.0.0]]
available = pairs.map do |current, latest|
  AppUpdate.define_singleton_method(:current_version) { current }
  Rails.cache.write(AppUpdate::CACHE_KEY, { version: latest, url: "x/#{latest}" })
  result = begin
    AppUpdate.available?
  rescue StandardError => e
    "raises #{e.class}"
  end
  { 'current' => current, 'latest' => latest, 'available' => result }
end
AppUpdate.define_singleton_method(:current_version, original)

markers = [nil, '', 'docker', 'umbrel', 'desktop', 'hosted', 'compose', ' Docker ', 'DESKTOP', "desktop\n"]
switches = [nil, '', 'false', 'FALSE', ' false ', '0', 'no', 'off', 'f', 'n', 'true', '1', 'yes', 'on', 'garbage']
enabled = markers.product(switches).map do |marker, switch|
  with_env.({ 'DELTABADGER_PLATFORM' => marker, 'DELTABADGER_UPDATE_CHECK' => switch }) do
    { 'platform_marker' => marker, 'update_check' => switch, 'platform' => Installation.platform.to_s, 'enabled' => AppUpdate.enabled? }
  end
end

File.write(out, "#{JSON.pretty_generate({ 'sources' => sources, 'refresh' => refresh, 'silent' => silent, 'available' => available,
                                           'enabled' => enabled, 'cache_ttl_secs' => AppUpdate::CACHE_TTL.to_i,
                                           'compose_command' => AppUpdate::COMPOSE_COMMAND })}\n")
