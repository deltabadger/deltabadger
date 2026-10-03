# Records what the Rust crate in rust/ must reproduce, from the app's own code.
#   bin/rails runner script/rust/record_vectors.rb rust/tests/fixtures/ruby_vectors.json
# Inputs are fixed and non-secret because the output is committed. Re-run when Rails, bcrypt or rotp
# move (rust/tests/codec.rs pins activerecord, bcrypt, bigdecimal and rotp) or when an enum changes.
require 'json'
require 'bcrypt'

def action_transport_vectors
  # The real Rack parser, Rails request merge, controller allowlist and decorator chain.
  # ACTION_TRANSPORT_ONLY updates only this deterministic section; older encrypted vectors contain
  # random IVs and must not be churned by a transport-only recording.
  transport_cases = [
    ['query_body_precedence', 'bots_dca_multi_asset[label]=query&bots_dca_multi_asset[interval]=day', 'bots_dca_multi_asset[label]=body', nil],
    ['nested_merge', 'bots_dca_multi_asset[allocations][2]=20&bots_dca_multi_asset[allocations][1]=80', 'bots_dca_multi_asset[allocations][2]=30',
     nil],
    ['checkbox_forward', '', 'bots_dca_multi_asset[smart_intervaled]=0&bots_dca_multi_asset[smart_intervaled]=1', nil],
    ['checkbox_reverse', '', 'bots_dca_multi_asset[smart_intervaled]=1&bots_dca_multi_asset[smart_intervaled]=0', nil],
    ['allocation_order', '',
     'bots_dca_multi_asset[allocations][2]=20&bots_dca_multi_asset[allocations][1]=80&bots_dca_multi_asset[allocations][2]=30', nil],
    ['missing_root', '', '', nil],
    ['empty_root', '', 'bots_dca_multi_asset=', nil],
    ['wrong_root', '', 'bots_dca_index[quote_amount]=1', nil],
    ['scalar_then_hash', '', 'a=1&a[b]=2', nil],
    ['hash_then_scalar', '', 'a[b]=2&a=1', nil],
    ['array_then_hash', '', 'a[]=1&a[b]=2', nil],
    ['hash_then_array', '', 'a[b]=1&a[]=2', nil],
    ['array_of_hashes', '', 'a[][x]=1&a[][y]=2&a[][x]=3&bots_dca_multi_asset[label]=ok', nil],
    ['nested_arrays', '', 'a[][]=1&bots_dca_multi_asset[label]=ok', nil],
    ['strong_shapes', '', nil,
     { 'bots_dca_multi_asset' => { 'label' => ['bad'], 'quote_amount' => { 'x' => 1 }, 'allocations' => { '2' => 20 }, 'unknown' => 'ignored' } }],
    ['typed_scalars', '', nil, { 'bots_dca_multi_asset' => { 'smart_intervaled' => true, 'quote_amount' => 12.5, 'label' => false } }],
    ['string_scalars', '', nil, { 'bots_dca_multi_asset' => { 'smart_intervaled' => 'true', 'quote_amount' => '12.5', 'label' => 'false' } }],
    ['null_scalar', '', nil, { 'bots_dca_multi_asset' => { 'label' => nil, 'quote_amount' => nil } }],
    ['allocations_array', '', nil, { 'bots_dca_multi_asset' => { 'allocations' => [20, 80], 'label' => 'ok' } }],
    ['allocations_nested', '', nil, { 'bots_dca_multi_asset' => { 'allocations' => { '2' => { 'x' => 20 }, '1' => [80] }, 'label' => 'ok' } }],
    ['invalid_json', '', '{', :raw_json]
  ]
  {
    'allowlists' => [Bots::DcaMultiAsset, Bots::DcaIndex].to_h do |klass|
      root = klass.model_name.param_key
      keys = klass.stored_attributes[:settings].map(&:to_s)
      keys += %w[label exchange_id]
      keys += if klass == Bots::DcaMultiAsset
                %w[add_asset_id remove_asset_id
                   normalize_allocations] + BotsController::BUY_TRIGGER_MODE_KEYS + BotsController::SELL_TRIGGER_MODE_KEYS
              else
                %w[
                  num_coins_ceiling num_coins_rendered
                ]
              end
      keys -= %w[allocations base_asset_ids] if klass == Bots::DcaMultiAsset
      [root, keys.uniq]
    end,
    'cases' => transport_cases.map do |name, query, form, json|
      row = { 'name' => name, 'query' => query, 'form' => form, 'json' => json == :raw_json ? nil : json }
      unless json
        begin
          row['rack_body'] = Rack::Utils.parse_nested_query(form)
        rescue Rack::QueryParser::ParameterTypeError => e
          row['rack_exception'] = e.class.name
        end
      end
      begin
        media = json ? 'application/json' : 'application/x-www-form-urlencoded'
        input = json == :raw_json ? form : json&.to_json || form
        env = Rack::MockRequest.env_for("http://localhost/bots/1?#{query}", method: 'PATCH', input:, 'CONTENT_TYPE' => media)
        request = ActionDispatch::Request.new(env)
        merged = request.parameters
        row['merged'] = merged
        controller = BotsController.new
        controller.params = ActionController::Parameters.new(merged)
        controller.instance_variable_set(:@bot, Bots::DcaMultiAsset.new(settings: { 'allocations' => {}, 'interval' => 'day', 'quote_amount' => 60 }))
        row['permitted'] = controller.send(:dca_multi_asset_bot_params).to_h
        row['updated'] = controller.send(:update_params)
      rescue StandardError => e
        row['exception'] = e.class.name
        row['bounded_input_divergence'] = true if e.is_a?(ActionController::BadRequest) ||
                                                  e.is_a?(ActionDispatch::Http::Parameters::ParseError)
      end
      row
    end
  }
end

# Replace one top-level pretty-printed object while preserving every other group's bytes,
# including randomized ciphertexts and JSON number classes from earlier recorders.
def write_vector_group(path, key, group)
  previous = File.read(path)
  JSON.parse(previous)
  encoded = JSON.pretty_generate(group).lines.each_with_index.map { |line, index| index.zero? ? line : "  #{line}" }.join
  marker = "\n  #{key.to_json}: "
  start = previous.index(marker)
  if start
    start += marker.length
    depth = 0
    quoted = false
    escaped = false
    finish = nil
    previous.each_char.with_index do |char, index|
      next if index < start

      if quoted
        if escaped
          escaped = false
        elsif char == '\\'
          escaped = true
        elsif char == '"'
          quoted = false
        end
      elsif char == '"'
        quoted = true
      elsif ['{', '['].include?(char)
        depth += 1
      elsif ['}', ']'].include?(char)
        depth -= 1
        if depth.zero?
          finish = index + 1
          break
        end
      end
    end
    raise "unterminated vector group #{key}" unless finish

    File.write(path, previous[0...start] + encoded + previous[finish..])
  else
    File.write(path, "#{previous.sub(/\n}\s*\z/, '')},#{marker}#{encoded}\n}\n")
  end
end

# Task 3 records through the real decorator chain and validation callbacks, on scratch rows only.
require 'active_support/testing/time_helpers'

module BotActionVectors
  extend ActiveSupport::Testing::TimeHelpers

  def self.record
    require_relative 'pages_bots'
    raise 'bot_actions needs an empty scratch database' unless Bot.count.zero? && User.count.zero?

    ActiveJob::Base.queue_adapter = :test
    now = Time.utc(2026, 9, 10, 12, 0, 30, 123_456)
    result = nil
    travel_to(now, with_usec: true) do
      ActiveRecord::Base.transaction(requires_new: true) do
        User.create!(name: 'Fixture Owner', email: 'owner@example.com', password: 'Password-Fixture1!', time_zone: 'Eastern Time (US & Canada)',
                     wash_sale_enabled: false)
        Pages.alpaca({})
        basket = Pages.bot('kind' => 'basket', 'columns' => { 'status' => 'stopped' })
        index = Pages.bot('kind' => 'index', 'columns' => { 'status' => 'stopped' })
        single = Pages.bot('kind' => 'single', 'columns' => { 'status' => 'stopped' }, 'orders' => 'one_fill')
        Pages.orders(single, 'open')
        single.update_columns(transient_data: single.transient_data.merge('quote_amount_limit_enabled_at' => '2026-09-01T00:00:00.000Z'))
        crypto = Pages.bot('kind' => 'coins', 'settings' => { 'allocations' => { Pages.asset_id('BTC').to_s => 1.0 } },
                           'columns' => { 'status' => 'stopped' })
        rows = %w[users exchanges assets exchange_assets tickers indices bots bot_index_assets transactions].to_h do |table|
          [table, ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a]
        end
        cases = []
        samples = { 'absent' => :absent, 'empty' => '', 'space' => '  ', 'normal' => '12.5', 'lower' => '0', 'upper' => '100',
                    'below' => '-0.0001', 'above' => '100.0001', 'object' => { 'bad' => '1' }, 'array' => ['1'],
                    'prefix' => '12.5abc', 'underscore' => '1_000', 'exponent' => '1e2', 'comma' => '12,5',
                    'infinity' => 'Infinity', 'nan' => 'NaN', 'overflow' => '1e999', 'true' => 'true', 'TRUE' => 'TRUE',
                    'false' => false, 'null' => nil }
        emit = lambda do |fixture, name, submitted, context, overrides = {}|
          record = Bot.find(fixture.id)
          record.assign_attributes(overrides.fetch(:original, {}))
          # Preserve the original status in ActiveRecord's dirty tracking, including archived Start.
          if overrides[:status]
            record.update_columns(status: overrides[:status])
            record = Bot.find(record.id)
          end
          raw = record.settings_in_database.deep_dup
          baseline = record.settings.deep_dup
          record.settings = record.settings.merge(overrides.fetch(:stored, {}))
          record.transient_data = record.transient_data.merge(overrides.fetch(:transient, {}))
          controller = BotsController.new
          controller.instance_variable_set(:@bot, record)
          controller.params = ActionController::Parameters.new(record.model_name.param_key => submitted.merge('unknown' => 'ignored'))
          row = { 'name' => name, 'bot_id' => fixture.id, 'context' => context.to_s, 'raw' => raw, 'baseline' => baseline,
                  'submitted' => submitted, 'stored' => overrides.fetch(:stored, {}), 'transient' => overrides.fetch(:transient, {}),
                  'persisted_status' => record.status_before_type_cast, 'zone' => record.user.time_zone,
                  'provider' => MarketData.configured?, 'delisted' => overrides.fetch(:delisted, false) }
          begin
            permitted = controller.send(record.is_a?(Bots::DcaIndex) ? :dca_index_bot_params : :dca_multi_asset_bot_params)
            row['permitted'] = permitted.to_h
            row['parsed'] = record.parse_params(permitted).stringify_keys
            record.assign_attributes(controller.send(:update_params))
            record.status = :scheduled if context == :start
            row['valid'] = record.valid?(context)
            row['candidate'] = record.settings.deep_dup
            row['candidate_status'] = record.status_before_type_cast
            row['label'] = record.label
            row['exchange_id'] = record.exchange_id
            row['errors'] = record.errors.messages.flat_map do |field, messages|
              messages.map do |message|
                { 'field' => field.to_s, 'message' => message }
              end
            end
            row['sentence'] = record.errors.messages.values.flatten.to_sentence
            row['settings_changed'] = record.settings_changed_since_load?
            if row['valid']
              record.send(:set_tickers) if record.will_save_change_to_exchange_id?
              %w[quote_amount_limit base_amount_limit price_limit price_drop_limit moving_average_limit indicator_limit].each do |prefix|
                if record.will_save_change_to_settings? && record.respond_to?("set_#{prefix}_enabled_at", true)
                  record.send("set_#{prefix}_enabled_at")
                  record.send("set_#{prefix}_condition_met_at") if record.respond_to?("set_#{prefix}_condition_met_at", true)
                end
                if record.will_save_change_to_exchange_id? && record.respond_to?("set_#{prefix}_in_ticker_id", true)
                  record.send("set_#{prefix}_in_ticker_id")
                end
              end
              record.send(:set_price_limit_value_condition) if record.will_save_change_to_settings? && record.respond_to?(
                :set_price_limit_value_condition, true
              )
              row['save_settings'] = (record.will_save_change_to_settings? ? record.settings : raw).deep_dup
              row['save_transient'] = record.transient_data.as_json
            end
          rescue StandardError => e
            row['exception'] = e.class.name
          end
          clean = lambda do |value|
            case value
            when Hash then value.transform_values { |child| clean.call(child) }
            when Array then value.map { |child| clean.call(child) }
            when Float then value.finite? ? value : { 'nonfinite' => value.to_s }
            else value
            end
          end
          cases << clean.call(row)
          fixture.update_columns(status: :stopped) if overrides[:status]
        end
        [basket, index].each do |fixture|
          keys = Object.new.send(:action_transport_vectors).fetch('allowlists').fetch(fixture.model_name.param_key)
          keys += ['allocations'] if fixture == basket
          keys.each do |key|
            samples.each do |sample, value|
              input = value == :absent ? {} : { key => value }
              input = { key => { '2' => value, '3' => '50' } } if key == 'allocations' && !%w[absent object array null].include?(sample)
              %i[update start].each { |context| emit.call(fixture, "#{fixture.id}/#{key}/#{sample}/#{context}", input, context) }
            end
          end
          # Each default validator also runs with the field unsubmitted. This records real grouping,
          # duplicate messages and the model's effective readers, rather than hand-written messages.
          fixture.class.validators.flat_map(&:attributes).uniq.each do |key|
            next unless fixture.class.stored_attributes[:settings].include?(key)

            %i[update start].each do |context|
              emit.call(fixture, "#{fixture.id}/stored/#{key}/#{context}", {}, context, stored: { key.to_s => 'invalid' })
            end
          end
          %w[created scheduled stopped executing waiting retrying archived].each do |status|
            %i[update start].each do |context|
              emit.call(fixture, "#{fixture.id}/status/#{status}/#{context}",
                        { 'interval' => 'week', 'exchange_id' => '2', 'weighting' => 'market_cap' }, context, status:)
            end
          end
        end
        %w[2026-03-08T02:30 2026-11-01T01:30 2026-12-01T15:00:00+02:00 2026-99-99T00:00].each do |date|
          %i[update start].each do |context|
            emit.call(basket, "date/#{date}/#{context}", { 'start_time_enabled' => 'true', 'start_time_mode' => 'date', 'start_at' => date }, context)
          end
        end
        %i[update start].each do |context|
          emit.call(basket, "pending/#{context}", { 'allocations' => { '2' => '20', '3' => '80' } }, context,
                    transient: { 'rebalance_pending' => { 'phase' => 'buying' } })
          emit.call(basket, "structural/#{context}",
                    { 'allocations' => { '2' => '20', '3' => '20' }, 'add_asset_id' => '4', 'remove_asset_id' => '3',
                      'normalize_allocations' => 'true' }, context)
        end
        numeric_rules = {
          'limit_order_pcnt_distance' => 'limit_ordered', 'quote_amount_limit' => 'quote_amount_limited',
          'price_limit' => 'price_limited', 'price_limit_range_lower_bound' => 'price_limited',
          'price_limit_range_upper_bound' => 'price_limited',
          'price_drop_limit' => 'price_drop_limited', 'moving_average_limit_in_period' => 'moving_average_limited',
          'indicator_limit' => 'indicator_limited'
        }
        numeric_rules.merge!(numeric_rules.except('limit_order_pcnt_distance', 'quote_amount_limit').transform_keys { |key| "sell_#{key}" }
                                         .transform_values { |key| "sell_#{key}" })
        numeric_rules.each do |field, flag|
          %w[-1 0 0.0001 1 100 100.01 invalid].each do |value|
            %i[update start].each do |context|
              emit.call(basket, "enabled/#{field}/#{value}/#{context}", { field => value, flag => 'true' }, context)
            end
          end
        end
        [basket, single, crypto, index].each do |fixture|
          [0.000001, 0.01, 1, 10_000].each do |amount|
            %i[update start].each do |context|
              emit.call(fixture, "floor/#{fixture.id}/#{amount}/#{context}",
                        { 'smart_intervaled' => 'true', 'smart_interval_quote_amount' => amount.to_s }, context)
            end
          end
        end
        %i[update start].each do |context|
          emit.call(single, "orders/quote/#{context}", { 'quote_asset_id' => '2' }, context)
          emit.call(single, "orders/exchange/#{context}", { 'exchange_id' => '2' }, context)
          emit.call(single, "cap/spent/#{context}", { 'quote_amount_limit' => '1' }, context)
          emit.call(single, "cap/remaining/#{context}", { 'quote_amount_limit' => '1000' }, context)
          emit.call(basket, "members/empty/#{context}", {}, context, stored: { 'allocations' => {} })
          emit.call(basket, "members/too_many/#{context}", {}, context, stored: { 'allocations' => (1..101).to_h { |id| [id.to_s, 1.0 / 101] } })
          emit.call(basket, "subject/removed/#{context}", { 'remove_asset_id' => '3', 'price_limited' => 'true', 'price_limit_in_ticker_id' => '3' },
                    context)
          emit.call(basket, "pending/exchange/#{context}", { 'exchange_id' => '2' }, context,
                    transient: { 'rebalance_pending' => { 'phase' => 'buying' } })
          emit.call(basket, "normalize/tie/#{context}",
                    { 'allocations' => { '4' => '20', '3' => '20', '2' => '20' }, 'normalize_allocations' => 'true' }, context)
          emit.call(basket, "weighting/derived/#{context}", {}, context,
                    stored: { 'weighting' => 'market_cap', 'allocations' => { '2' => 0.0, '3' => 0.0 } })
          %w[hour monday date invalid].each do |mode|
            %w[09:30 25:60].each do |time|
              emit.call(basket, "starting/#{mode}/#{time}/#{context}",
                        { 'start_time_enabled' => 'true', 'start_time_mode' => mode, 'start_time_of_day' => time }, context)
            end
          end
          %w[quote base].each do |denomination|
            [nil, 0, 0.0000000001, 1].each do |amount|
              emit.call(crypto, "selling/#{denomination}/#{amount}/#{context}", {}, context,
                        stored: { 'direction' => 'selling', 'sell_denomination' => denomination, 'sell_amount' => amount,
                                  'smart_intervaled' => true, 'base_amount_limited' => true, 'base_amount_limit' => 0.0 })
            end
          end
          emit.call(index, "index/working_composition/#{context}", { 'num_coins' => '5' }, context, status: 'waiting')
          emit.call(index, "index/pending_composition/#{context}", { 'allocation_flattening' => '0.5' }, context,
                    transient: { 'rebalance_pending' => { 'phase' => 'buying' } })
        end
        # Delist after the baseline fixtures: both a persisted page and a Start see the changed row.
        Ticker.where(exchange_id: basket.exchange_id, base: 'QQQM').update_all(available: false)
        %i[update start].each { |context| emit.call(basket, "delisted/#{context}", {}, context, delisted: true) }
        Ticker.where(exchange_id: basket.exchange_id, base: 'QQQM').update_all(available: true)
        AppConfig.set('market_data_token', '')
        %i[update start].each { |context| emit.call(index, "provider/#{context}", {}, context, provider: false) }
        AppConfig.set('market_data_token', 'token')
        basket.update_columns(settings: basket.settings.except('smart_intervaled', 'quote_amount_limited', 'price_limited', 'limit_ordered'))
        %i[update start].each { |context| emit.call(basket, "defaults/#{context}", {}, context) }
        raise 'empty bot_actions vectors' if cases.empty?

        # Main's index model permits these changes; the plan deliberately freezes them. Reuse
        # the actual basket field error rather than writing a Rust expected message by hand.
        lock_error = cases.find { |row| row['name'] == '1/status/scheduled/update' }.fetch('errors')
                          .find { |error| error['field'] == 'allocations' }
        raise 'missing Rails composition lock error' unless lock_error

        cases.each do |row|
          next unless row['bot_id'] == index.id && row['context'] == 'update' && row['candidate']
          next unless %w[scheduled executing waiting
                         retrying].include?(Bot.statuses.key(row['persisted_status'])) || row['transient']['rebalance_pending'].present?

          changed = %w[num_coins hold_all index_type index_category_id quote_asset_id allocation_flattening]
                    .any? { |key| row['raw'][key] != row['candidate'][key] }
          next unless changed || row['exchange_id'] != index.exchange_id

          row['rust_lock_error'] = lock_error.deep_dup
          row['rust_sentence'] = (row['errors'].map { |error| error['message'] } + [lock_error['message']]).to_sentence
        end
        # Intern complete settings snapshots by their JSON bytes (not Hash equality: 1 and 1.0
        # compare equal in Ruby). Cases still reference exact raw, loaded and candidate snapshots.
        states = []
        state_ids = {}
        cases.each do |row|
          %w[raw baseline candidate save_settings save_transient].each do |key|
            next unless row.key?(key)

            bytes = JSON.generate(row.fetch(key))
            row[key] = state_ids.fetch(bytes) do
              id = states.size
              states << row.fetch(key)
              state_ids[bytes] = id
            end
          end
        end
        result = { 'now' => now.iso8601(6), 'rows' => rows, 'count' => cases.size, 'states' => states, 'cases' => cases }
        raise ActiveRecord::Rollback
      end
    end
    result
  end
end

# Startable's pure first-anchor calculation, independent of the engine's scope refusal.
# Record only this group so encrypted fixtures and earlier action vectors retain their bytes.
if ENV['ACTION_LIFECYCLE_ONLY'] == 'true'
  inputs = [
    ['UTC', '2026-09-10T12:00:30.123456Z', 'date', '2026-09-11T13:45:00Z'],
    ['UTC', '2026-09-10T12:00:30.123456Z', 'hour', '13:45'],
    ['UTC', '2026-09-10T14:00:00Z', 'hour', '13:45'],
    ['Warsaw', '2026-09-10T12:00:30.123456Z', 'monday', '09:30'],
    ['Eastern Time (US & Canada)', '2026-03-08T06:00:00Z', 'hour', '02:30'],
    ['Eastern Time (US & Canada)', '2026-11-01T04:00:00Z', 'hour', '01:30'],
    ['Eastern Time (US & Canada)', '2026-03-06T12:00:00Z', 'monday', '09:30']
  ]
  group = inputs.map do |zone, clock, mode, value|
    settings = { 'start_time_enabled' => true, 'start_time_mode' => mode,
                 mode == 'date' ? 'start_at' : 'start_time_of_day' => value }
    bot = Bots::DcaMultiAsset.new(user: User.new(time_zone: zone), settings:)
    { 'zone' => zone, 'now' => clock, 'settings' => settings,
      'expected' => bot.initial_start_at(now: Time.iso8601(clock))&.utc&.iso8601 }
  end
  path = Rails.root.join('rust/tests/fixtures/ruby_vectors.json')
  write_vector_group(path, 'action_lifecycle', group)
  puts "wrote #{group.size} action_lifecycle vectors in #{path}"
  exit
end

# The task's no-argument command adds only the new group. Earlier recordings contain random
# encryption material; preserve their bytes and avoid writing unrelated generated zone files.
if ARGV.empty?
  path = Rails.root.join('rust/tests/fixtures/ruby_vectors.json')
  group = BotActionVectors.record
  write_vector_group(path, 'bot_actions', group)
  puts "wrote #{group.fetch('count')} bot_actions vectors in #{path}"
  exit
end

if ENV['ACTION_TRANSPORT_ONLY'] == 'true'
  write_vector_group(ARGV.fetch(0), 'action_transport', action_transport_vectors)
  puts "wrote action_transport in #{ARGV.fetch(0)}"
  exit
end

SECRET = 'rust-fixture-secret-key-base'.freeze

# The key Rails derives from a primary key + salt, computed explicitly so no global encryption
# config is mutated. check_against_app! proves this path equals the app's configured one.
def key_provider(primary, salt)
  bytes = ActiveSupport::KeyGenerator.new(primary, hash_digest_class: OpenSSL::Digest::SHA256, iterations: 2**16)
                                     .generate_key(salt, 32)
  ActiveRecord::Encryption::KeyProvider.new(ActiveRecord::Encryption::Key.new(bytes))
end

def encrypt_with(provider, model, attribute, plain)
  ActiveRecord::Encryption.with_encryption_context(key_provider: provider) do
    model.type_for_attribute(attribute).serialize(plain)
  end
end

def check_against_app!
  config = ActiveRecord::Encryption.config
  provider = key_provider(Array(config.primary_key).first, config.key_derivation_salt)
  cipher = ApiKey.type_for_attribute(:secret).serialize('probe') # the app's real attribute path
  plain = ActiveRecord::Encryption.with_encryption_context(key_provider: provider) do
    ApiKey.type_for_attribute(:secret).deserialize(cipher)
  end
  raise 'explicit key derivation differs from the app configuration' unless plain == 'probe'
end

check_against_app!
primary = EncryptionKeys.derived_primary_key(SECRET)
salt = EncryptionKeys.derived_salt(SECRET)
provider = key_provider(primary, salt)
long = "-----BEGIN PRIVATE KEY-----\n#{'MIIEvQIBADANBgkqhkiG9w0BAQEFAASC' * 12}\n-----END PRIVATE KEY-----"
plains = {
  'short' => [ApiKey, :secret, 'kR4k3n-s3cr3t/+=='],
  'unicode' => [ApiKey, :key, 'zażółć 🦡'],
  'compressed' => [ApiKey, :rsa_signature_key, long],
  # Through the app's own path: ROTP returns a binary string, so Rails adds an "e" (encoding) header.
  'otp_seed' => [User, :otp_secret_key, User.new.tap(&:otp_regenerate_secret).otp_secret_key],
  'app_config' => [AppConfig, :value, '{"engine":"rust"}']
}
passwords = ['correct horse ☃', 'x' * 100] # the second is past bcrypt's 72-byte limit
totp_times = [0, 59, 1_790_000_000, 1_790_000_029, 1_790_000_030]
times = [Time.utc(2026, 9, 28, 22, 1, 3, 734_946), Time.utc(2026, 9, 28, 22, 1, 3)]
rng = Random.new(42)
floats = [30.0003, 0.1 + 0.2, 72_960.5, 0.00041118, 1.0e-08, 1.0e-18, 123_456_789.12345679, 1.0 / 3, 60.0, -2.5,
          99_999_999_999_999_999.0] + Array.new(300) { (rng.rand * (10**rng.rand(-12..12))).round(rng.rand(0..17)) }
decimal = Transaction.type_for_attribute(:amount)

vectors = {
  'secret_key_base' => SECRET,
  'primary_key' => primary,
  'key_derivation_salt' => salt,
  'ciphertexts' => plains.transform_values { |(model, attr, plain)| { 'plain' => plain, 'cipher' => encrypt_with(provider, model, attr, plain) } },
  'bcrypt' => passwords.map { |pw| { 'password' => pw, 'hash' => BCrypt::Password.create(pw, cost: 11).to_s } },
  'totp' => { 'seed' => 'JBSWY3DPEHPK3PXPJBSWY3DP', 'codes' => totp_times.map { |t| [t, ROTP::TOTP.new('JBSWY3DPEHPK3PXPJBSWY3DP').at(t)] } },
  'times' => times.map { |t| [t.iso8601(6), Transaction.connection.quoted_date(t)] },
  # IEEE bits, not JSON numbers: Ruby's JSON.generate prints 123456789.12345679 as 123456789.1234568.
  'decimals' => floats.map { |f| [[f].pack('G').unpack1('H*'), decimal.cast(f).to_s('F')] },
  # Floats outside rust_decimal's range (scale > 28, >= ~1e29): Ruby reads them fine, and so must BigDec.
  'decimals_wide' => [1.234567890123456e-14, 3.0e-20, 1.5e30, -2.5e-15, 1.0e-30, 123_456_789_012_345.6e20, 5.0e-324, 1.7976931348623157e308]
                       .map { |f| [[f].pack('G').unpack1('H*'), decimal.cast(f).to_s('F')] },
  'enums' => {
    'bot_status' => Bot.statuses, 'rule_status' => Rule.statuses, 'transaction_status' => Transaction.statuses, 'transaction_side' => Transaction.sides,
    'transaction_order_type' => Transaction.order_types, 'transaction_external_status' => Transaction.external_statuses,
    'api_key_status' => ApiKey.statuses, 'api_key_key_type' => ApiKey.key_types, 'user_otp_module' => User.otp_modules
  },
  'gems' => %w[activerecord bcrypt bigdecimal rotp actionpack actionview activesupport devise i18n rack-attack turbo-rails rack puma].to_h { |g| [g, Gem.loaded_specs.fetch(g).version.to_s] }
}
# Ruby BigDecimal, as Rails computes with it (rust/src/ruby.rs BigDec). Seeded, so the file is stable.
bd_rng = Random.new(11)
bd_num = lambda do
  digits = Array.new(bd_rng.rand(1..20)) { bd_rng.rand(10).to_s }.join.sub(/\A0+/, '')
  BigDecimal("#{bd_rng.rand < 0.1 ? '-' : ''}0.#{digits.empty? ? '1' : digits}e#{bd_rng.rand(-15..12)}")
end
bigdec = []
[%w[0.4 50000.2], %w[60 50000.2], %w[60 49870.0], %w[1 3], %w[2 3], %w[100 7],
 %w[0.000007999968000127999488002047991808 50000.2], %w[5 0.5], %w[123456789012345678901234567890 7]]
  .each { |a, b| bigdec << ['div', a, b, (BigDecimal(a) / BigDecimal(b)).to_s('F')] }
3000.times { a = bd_num.(); b = bd_num.(); bigdec << ['div', a.to_s('F'), b.to_s('F'), (a / b).to_s('F')] }
500.times { a = bd_num.(); b = bd_num.(); bigdec << ['mul', a.to_s('F'), b.to_s('F'), (a * b).to_s('F')] }
500.times { a = bd_num.(); b = bd_num.(); bigdec << ['add', a.to_s('F'), b.to_s('F'), (a + b).to_s('F')] }
500.times { a = bd_num.(); b = bd_num.(); bigdec << ['sub', a.to_s('F'), b.to_s('F'), (a - b).to_s('F')] }
500.times { a = bd_num.(); bigdec << ['to_f', a.to_s('F'), '', [a.to_f].pack('G').unpack1('H*')] }
500.times { a = bd_num.(); bigdec << ['precision', a.to_s('F'), '', a.precision.to_s] }
[['0.123456789', 8], ['1.99999', 2], ['100', 5], ['0.00049999', 4], ['7.5', 0], ['-0.5', 1], ['49870.0125', 1], ['0.9975', 0]].each do |s, n|
  bigdec << ['floor', s, n.to_s, BigDecimal(s).floor(n).to_d.to_s('F')]
  bigdec << ['ceil', s, n.to_s, BigDecimal(s).ceil(n).to_d.to_s('F')]
end
%w[0.1234567890123456785 0.1234567890123456784 1.0000000000000000005 -0.0000000000000000005].each do |s|
  bigdec << ['round18', s, '', BigDecimal(s).round(18).to_s('F')]
end
vectors['bigdec'] = bigdec
vectors['ruby'] = {
  'iso8601_ms' => [Time.utc(2026, 3, 8, 7, 27, 44, 458_901), Time.utc(2026, 3, 8, 7, 27, 44)].map { |t| [t.iso8601(6), t.as_json] },
  # anchor + offset, then Time#round(6); offsets as IEEE bits so the float is exact on both sides.
  'round6' => [[Time.utc(2026, 9, 1, 10, 0, 0, 123_456), 604_800.0 / 3 * 7],
               [Time.utc(2026, 9, 1, 10, 0, 0), 0.0000005], [Time.utc(2026, 9, 1, 10, 0, 0), 0.0000004999],
               [Time.utc(2026, 9, 1, 10, 0, 0, 1), 86_400.0 / 7]]
                .map { |t, off| [t.iso8601(6), [off].pack('G').unpack1('H*'), (t + off).round(6).iso8601(6)] },
  # Several terms, as Ruby does it: one Time +/- Float per repetition (negative k subtracts).
  'round6_multi' => [
    [Time.utc(2026, 9, 1, 10, 0, 0, 123_456), [[604_800.0 / 3 * 7, 3]]],
    [Time.utc(2026, 9, 1, 10, 0, 0, 999_999), [[86_400.0 / 7, -2]]],
    [Time.utc(2026, 9, 1, 10, 0, 0), [[0.1, 1], [86_400.0 / 7, 2]]],
    [Time.utc(2026, 9, 1, 10, 0, 0, 5), [[0.0000005, 1], [-0.0000004999, 1], [3600.0 / 11, -3]]],
    [Time.utc(2026, 9, 1, 10, 0, 0), [[1.0 / 3, 3], [0.2, 5]]]
  ].map do |t, terms|
    r = terms.reduce(t) { |acc, (f, k)| k.abs.times.reduce(acc) { |a, _| k.positive? ? a + f : a - f } }
    [t.iso8601(6), terms.map { |f, k| [[f].pack('G').unpack1('H*'), k] }, r.round(6).iso8601(6)]
  end,
  # exceeds: anchor + k*f > now, on the exact boundary and one microsecond either side.
  'exceeds' => [[Time.utc(2026, 9, 1, 10, 0, 0), 90.5, 3], [Time.utc(2026, 9, 1, 10, 0, 0, 250_000), 0.25, 4],
                [Time.utc(2026, 9, 1, 10, 0, 0, 123_456), 86_400.0 / 7, 1], [Time.utc(2026, 9, 1, 10, 0, 0), 0.1, 3]].flat_map do |t, f, k|
    due = k.times.reduce(t) { |a, _| a + f }
    [-1, 0, 1].map do |d|
      now = due.round(6) + Rational(d, 1_000_000)
      [t.iso8601(6), [f].pack('G').unpack1('H*'), k, now.iso8601(6), due > now]
    end
  end,
  'to_sentence' => [%w[a], %w[a b], %w[a b c]].map { |a| [a, a.to_sentence] },
  'inspect' => [['EGeneral:Internal error'], ['EOrder:Insufficient funds', 'a "quoted" one']].map { |a| [a, a.inspect] }
}
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers
# Unsaved bots: the schedule only reads started_at and settings. Every case is recorded through the real
# model methods (Automation::Schedulable, Bot::SmartIntervalable, Bot::Accountable's interval count).
schedule_cases = []
anchors = [Time.utc(2026, 1, 31, 10, 0, 0, 123_456), Time.utc(2026, 9, 1, 22, 1, 3), Time.utc(2026, 2, 28, 23, 59, 59, 999_999)]
settings = [
  { 'interval' => 'hour', 'quote_amount' => 10.0 }, { 'interval' => 'day', 'quote_amount' => 10.0 },
  { 'interval' => 'week', 'quote_amount' => 60.0 }, { 'interval' => 'month', 'quote_amount' => 100.0 },
  { 'interval' => 'week', 'quote_amount' => 60.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 30.0 },
  { 'interval' => 'week', 'quote_amount' => 100.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 7.0 },
  { 'interval' => 'month', 'quote_amount' => 100.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 100.0 },
  { 'interval' => 'month', 'quote_amount' => 100.0, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 33.0 },
  { 'interval' => 'day', 'quote_amount' => 10.0, 'smart_intervaled' => false, 'smart_interval_quote_amount' => 3.0 }
]
offsets = [-3600, 0, 1, 3599.9999995, 86_400 * 3 + 7, 86_400 * 45, 86_400 * 400 + 0.5]
record = lambda do |anchor, s, now, bot|
  travel_to(now, with_usec: true) do
    nxt = bot.next_interval_checkpoint_at
    last = bot.last_interval_checkpoint_at
    count = ((last.round(6) - anchor.round(6)) / bot.effective_interval_duration).floor + 1
    schedule_cases << { 'anchor' => anchor.iso8601(6), 'now' => now.iso8601(6), 'settings' => s,
                        'next' => nxt.round(6).iso8601(6), 'last' => last.round(6).iso8601(6), 'count' => count }
    nxt
  end
end
anchors.each do |anchor|
  settings.each do |s|
    bot = Bots::DcaMultiAsset.new(started_at: anchor, settings: s)
    offsets.each { |off| record.call(anchor, s, anchor + off, bot) }
    # On the grid: the bot's own first two checkpoints after the anchor, each exactly on, and one microsecond either side.
    grid = []
    point = anchor
    2.times { point = travel_to(point + Rational(1, 1_000_000), with_usec: true) { bot.next_interval_checkpoint_at.round(6) }; grid << point }
    grid.each { |g| [-1, 0, 1].each { |d| record.call(anchor, s, g + Rational(d, 1_000_000), bot) } }
  end
end
vectors['schedule'] = schedule_cases
kraken = Exchanges::Kraken.new
sizing = []
sizing_tickers = [
  { minimum_base_size: '0.00005', minimum_quote_size: '0.5', base_decimals: 8, quote_decimals: 5, price_decimals: 1 },
  { minimum_base_size: '0.0001', minimum_quote_size: '5', base_decimals: 8, quote_decimals: 2, price_decimals: 2 },
  { minimum_base_size: '1', minimum_quote_size: '500', base_decimals: 0, quote_decimals: 0, price_decimals: 0 }
]
%w[50000.2 49995.37 0.00123456 1234567.8].each do |price_s|
  %w[60 0.4 5.00001 0.0001 123.456789].each do |x_s|
    sizing_tickers.each do |t|
      %i[market_order limit_order].each do |order_type|
        ticker = Ticker.new(exchange: kraken, **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
        bot = Bots::DcaMultiAsset.new(exchange: kraken)
        price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
        next if price.zero? # both refuse before sizing ("limit price rounds to zero..."), covered by unit tests on each side
        x = BigDecimal(x_s)
        info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
        volume = ticker.adjusted_amount(amount: info[:amount], amount_type: info[:amount_type])
        sizing << { 'ticker' => t.transform_values(&:to_s), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
                    'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
                    'below_minimum' => info[:below_minimum_amount], 'volume' => volume.to_d.to_s('F') }
      end
    end
  end
end
vectors['sizing'] = sizing
# Exchange#failure_kind / #transient_error? / #throttled_error? per venue (rust/src/engine/venue_rules.rs).
failure_messages = ['insufficient buying power', 'unauthorized.', 'HTTP 401', 'HTTP 403', 'forbidden.', 'rate limit exceeded',
                    'Faraday::ConnectionFailed: Connection refused - connect(2) for "paper-api.alpaca.markets" port 443',
                    'Faraday::TimeoutError: Net::ReadTimeout', 'internal server error', 'Connection reset by peer',
                    'EOrder:Insufficient funds', 'EAPI:Invalid key', 'EAPI:Rate limit exceeded', 'EService:Unavailable',
                    'EAPI:Invalid nonce', 'qty must be >= 0.000027', 'EAPI:Invalid signature', 'EGeneral:Permission denied',
                    'EAccount:Invalid permissions:USDT trading restricted for AT.', 'EGeneral:Internal error', 'EService:Busy',
                    'EService:Deadline elapsed']
vectors['failure_kinds'] = { 'Exchanges::Alpaca' => Exchanges::Alpaca.new, 'Exchanges::Kraken' => Exchanges::Kraken.new }.flat_map do |type, ex|
  failure_messages.map { |m| [type, m, ex.failure_kind([m])&.to_s, ex.transient_error?([m]), ex.throttled_error?([m])] }
end
# The Ruby the engine ports by hand (the Alpaca venue, the carry, and the two poll jobs that used to draw it down).
# rust/tests/venue_rules.rs fails when any of it changes, until re-recorded and re-checked: a later change to Rails' carry trips it.
vectors['ported_sources'] = %w[app/models/clients/alpaca.rb app/models/exchanges/alpaca.rb app/models/client.rb app/models/exchange.rb
                                 app/models/exchanges/kraken.rb app/models/bot/accountable.rb
                                 app/jobs/bot/fetch_and_update_open_orders_job.rb app/jobs/bot/fetch_and_update_order_job.rb]
                              .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }
# Exchanges::Alpaca sizing and the exact strings #set_market_order / #set_limit_order send (rust/src/engine/amount.rs).
alpaca = Exchanges::Alpaca.new
captured = nil
alpaca.instance_variable_set(:@client, Object.new.tap do |c|
  c.define_singleton_method(:create_order) { |**kw| captured = kw; Result::Success.new('id' => 'X') }
end)
crypto = Asset.new(symbol: 'BTC', category: 'Cryptocurrency') # crypto_ticker? reads the base asset's category
alpaca_sizing = []
alpaca_tickers = [
  { minimum_base_size: '0.000027', minimum_quote_size: '1', base_decimals: 9, quote_decimals: 2, price_decimals: 0 },
  { minimum_base_size: '0.0001', minimum_quote_size: '1', base_decimals: 4, quote_decimals: 2, price_decimals: 2 },
  { minimum_base_size: '1', minimum_quote_size: '10', base_decimals: 0, quote_decimals: 2, price_decimals: 5 },
  # >17 significant digits after flooring (qty ~8.1e8 at 9 decimals): the only ticker where Float formatting changes a string
  { minimum_base_size: '0.000000001', minimum_quote_size: '1', base_decimals: 9, quote_decimals: 2, price_decimals: 8 }
]
%w[64321.5 0.00123456 1.5 123456789012.12345].each do |price_s|
  %w[60 0.99 1 5.005 123.456789 1000000].each do |x_s|
    alpaca_tickers.each do |t|
      %i[market_order limit_order].each do |order_type|
        ticker = Ticker.new(exchange: alpaca, ticker: 'BTC/USD', base: 'BTC', quote: 'USD', base_asset: crypto,
                            **t.transform_values { |v| v.is_a?(String) ? BigDecimal(v) : v })
        bot = Bots::DcaMultiAsset.new(exchange: alpaca)
        price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
        next if price.zero? # both refuse before sizing ("limit price rounds to zero")
        x = BigDecimal(x_s)
        info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
        captured = nil
        if order_type == :limit_order
          alpaca.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
        else
          alpaca.market_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type])
        end
        alpaca_sizing << { 'ticker' => t.transform_values(&:to_s), 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s,
                           'price' => price.to_s('F'), 'amount' => (x / price).to_s('F'), 'amount_type' => info[:amount_type].to_s,
                           'below_minimum' => info[:below_minimum_amount], 'wire' => captured.transform_keys(&:to_s).transform_values(&:to_s) }
      end
    end
  end
end
vectors['alpaca_sizing'] = alpaca_sizing
# Exchanges::Alpaca#parse_order_data over documented order shapes and every status it maps (rust/src/venue/alpaca.rs).
alpaca_parser = Exchanges::Alpaca.new
order_shapes = [
  { 'type' => 'market', 'side' => 'buy', 'notional' => '60', 'qty' => nil, 'filled_qty' => '0', 'filled_avg_price' => nil, 'limit_price' => nil },
  { 'type' => 'market', 'side' => 'buy', 'notional' => '60', 'qty' => nil, 'filled_qty' => '0.000932719', 'filled_avg_price' => '64328.1', 'limit_price' => nil },
  { 'type' => 'limit', 'side' => 'buy', 'notional' => nil, 'qty' => '0.00093525', 'filled_qty' => '0.0004', 'filled_avg_price' => '64149.97', 'limit_price' => '64149.97' }
]
order_statuses = %w[new accepted pending_new filled canceled expired replaced rejected partially_filled done_for_day pending_cancel held mystery]
dec_s = ->(d) { d.nil? ? nil : d.to_d.to_s('F') }
vectors['alpaca_orders'] = order_statuses.product(order_shapes).map do |status, shape|
  body = shape.merge('id' => 'O1', 'symbol' => 'BTC/USD', 'status' => status)
  parsed = alpaca_parser.send(:parse_order_data, body)
  [body, { 'status' => parsed[:status].to_s, 'price' => dec_s.(parsed[:price]), 'amount' => dec_s.(parsed[:amount]),
           'quote_amount' => dec_s.(parsed[:quote_amount]), 'amount_exec' => dec_s.(parsed[:amount_exec]),
           'quote_amount_exec' => dec_s.(parsed[:quote_amount_exec]), 'order_type' => parsed[:order_type].to_s, 'side' => parsed[:side].to_s }]
end
# Numeric acceptance is part of the venue port too: missing fills must not become zero, and an
# unused limit price still has to parse. Keep optional nulls and legitimate unfilled orders accepted.
vectors['venue_order_numbers'] = {
  'alpaca' => [alpaca_parser, order_shapes[1].merge('id' => 'O1', 'symbol' => 'BTC/USD', 'status' => 'filled'),
               %w[filled_qty filled_avg_price notional qty limit_price]],
  'kraken' => [kraken, { 'status' => 'closed', 'price' => '50000', 'cost' => '60', 'vol' => '0.0012', 'vol_exec' => '0.0012',
                        'oflags' => '', 'descr' => { 'pair' => 'XBTEUR', 'type' => 'buy', 'ordertype' => 'market', 'price' => '0' } },
               %w[price cost vol vol_exec]]
}.flat_map do |venue, (exchange, original, fields)|
  bodies = [original]
  bodies << original.merge('filled_qty' => '0', 'filled_avg_price' => nil) if venue == 'alpaca'
  fields.each do |field|
    [nil, '', 'garbage', 'NaN', 'Infinity', '-Infinity', '0', '1.25'].each { |value| bodies << original.merge(field => value) }
    bodies << original.except(field)
  end
  bodies.map do |body|
    begin
      args = venue == 'alpaca' ? [body] : ['O1', body]
      exchange.send(:parse_order_data, *args)
      accepted = true
    rescue ArgumentError, KeyError
      accepted = false
    end
    { 'venue' => venue, 'body' => body, 'accepted' => accepted }
  end
end
# Bot::Accountable#pending_quote_amount with Rails' fill-credit fix: a cancelled or abandoned REGULAR
# buy counts what it filled, nothing in polling moves the carry, and every counted row comes from one read. Generated rows on a
# bot inserted directly, inside a transaction that is rolled back, so the development database keeps nothing.
CARRY_KINDS = %w[closed open_limit unknown_market cancelled_partial cancelled_unfilled abandoned_nil abandoned_partial
                 failed skipped sell rebalance before_window].freeze
carry_row = lambda do |kind, n|
  row = { 'status' => 0, 'side' => 0, 'transaction_type' => 'REGULAR', 'external_id' => "rust-carry-#{n}", 'order_type' => 0,
          'price' => '64150', 'created_at' => "2026-09-0#{2 + (n % 5)} 10:00:00" }
  case kind
  when 'closed' then row.merge('external_status' => 2, 'quote_amount' => '60', 'quote_amount_exec' => '59.97', 'amount_exec' => '0.000935')
  when 'open_limit' then row.merge('external_status' => 1, 'order_type' => 1, 'amount' => '0.000935', 'amount_exec' => '0.0003',
                                   'quote_amount_exec' => '19.245')
  when 'unknown_market' then row.merge('external_status' => 0, 'quote_amount' => '60')
  when 'cancelled_partial' then row.merge('external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '39.96', 'amount_exec' => '0.000623')
  when 'cancelled_unfilled' then row.merge('external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '0', 'amount_exec' => '0')
  when 'abandoned_nil' then row.merge('external_status' => 4, 'quote_amount' => '60')
  when 'abandoned_partial' then row.merge('external_status' => 4, 'quote_amount' => '60', 'quote_amount_exec' => '12.5', 'amount_exec' => '0.000195')
  when 'failed' then row.merge('status' => 1, 'external_id' => nil, 'quote_amount' => '60', 'quote_amount_exec' => '0', 'amount_exec' => '0')
  when 'skipped' then row.merge('status' => 2, 'external_id' => nil, 'quote_amount' => '0.4', 'quote_amount_exec' => '0', 'amount_exec' => '0')
  when 'sell' then row.merge('side' => 1, 'external_status' => 2, 'quote_amount' => '60', 'quote_amount_exec' => '60', 'amount_exec' => '0.000935')
  when 'rebalance' then row.merge('transaction_type' => 'REBALANCE', 'external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '30',
                                  'amount_exec' => '0.00047')
  when 'before_window' then row.merge('external_status' => 3, 'quote_amount' => '60', 'quote_amount_exec' => '40', 'amount_exec' => '0.000623',
                                      'created_at' => '2026-08-31 10:00:00')
  end
end
carry_rng = Random.new(2_202_615)
vectors['carry'] = Array.new(36) do |i|
  settings = { 'interval' => %w[day week].fetch(i % 2), 'quote_amount' => 60.0 }
  settings.merge!('smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0) if i % 6 == 5
  kinds = Array.new(carry_rng.rand(0..5)) { CARRY_KINDS.sample(random: carry_rng) }
  kinds |= ['cancelled_partial'] if i.even? # half the cases exercise the cancelled fill's credit
  c = { 'settings' => settings, 'started_at' => '2026-09-01 10:00:00', 'settings_changed_at' => (i % 4 == 3 ? '2026-09-03 12:00:00' : nil),
        'carry' => %w[0 12.5 100.0].fetch(i % 3), 'rows' => kinds.each_with_index.map { |k, n| carry_row.(k, (i * 10) + n) },
        'now' => (Time.utc(2026, 9, 1, 10) + ((i % 9) + 1).days + 1).iso8601(6) }
  ActiveRecord::Base.transaction do
    alpaca = Exchanges::Alpaca.first || Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    user = User.new(name: 'Carry', email: 'rust-carry@example.com', password: 'correct horse battery staple', confirmed_at: Time.current)
    user.save!(validate: false)
    stamp = Time.current
    id = Bot.insert!({ 'type' => 'Bots::DcaMultiAsset', 'label' => 'Carry', 'user_id' => user.id, 'exchange_id' => alpaca.id, 'status' => Bot.statuses[:scheduled],
                       'settings' => settings, 'transient_data' => { 'missed_quote_amount' => c['carry'] }, 'started_at' => c['started_at'],
                       'settings_changed_at' => c['settings_changed_at'], 'created_at' => stamp, 'updated_at' => stamp }).rows.first.first
    c['rows'].each do |r|
      Transaction.insert!(r.merge('bot_id' => id, 'exchange_id' => alpaca.id, 'bot_interval' => settings['interval'], 'bot_quote_amount' => 60,
                                  'error_messages' => [], 'updated_at' => r['created_at']))
    end
    c['pending'] = travel_to(Time.iso8601(c['now']), with_usec: true) { Bot.find(id).pending_quote_amount.to_d.to_s('F') }
    raise ActiveRecord::Rollback
  end
  c
end
# Alpaca crypto baskets (rust/src/engine/basket.rs), recorded on real rows inside a transaction that is rolled back,
# so the development database keeps nothing. Members are V-prefixed so no real asset, ticker or order id is touched.
Rails.cache = ActiveSupport::Cache::NullStore.new # metrics(force: true) recomputes; nothing is written to a cache store
BASKET_PAIRS = {
  'VBTC' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' },
  'VETH' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.0005', 'minimum_quote_size' => '1' },
  'VSOL' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 3, 'minimum_base_size' => '0.01', 'minimum_quote_size' => '1' },
  'VADA' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 4, 'minimum_base_size' => '1', 'minimum_quote_size' => '1' }
}.freeze
# A basket over `weights` on the development database's Alpaca exchange, saved as BotApi::Bots::Create saves one (its
# after_save refresh_composition writes bot_index_assets), with `rows` inserted as REGULAR buys. Yields it and its assets
# by symbol, returns the block's value, and rolls everything back.
def with_basket(weights, settings: {}, rows: [])
  out = nil
  ActiveRecord::Base.transaction do
    alpaca = Exchanges::Alpaca.first || Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    usd = Asset.create!(external_id: 'rust-vector-usd', symbol: 'USD', name: 'US Dollar', category: 'Currency')
    ExchangeAsset.create!(exchange: alpaca, asset: usd, available: true) # Ticker#exchange_matches_assets
    assets = weights.keys.to_h do |sym|
      asset = Asset.create!(external_id: "rust-vector-#{sym.downcase}", symbol: sym, name: sym, category: 'Cryptocurrency')
      ExchangeAsset.create!(exchange: alpaca, asset:, available: true)
      Ticker.create!(exchange: alpaca, ticker: "#{sym}/USD", base: sym, quote: 'USD', base_asset: asset, quote_asset: usd,
                     **BASKET_PAIRS.fetch(sym).to_h { |k, v| [k.to_sym, v.is_a?(String) ? BigDecimal(v) : v] })
      [sym, asset]
    end
    user = User.new(name: 'Vectors', email: 'rust-vectors@example.com', password: 'correct horse battery staple', confirmed_at: Time.current)
    user.save!(validate: false)
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange: alpaca, settings: {
      'quote_asset_id' => usd.id, 'quote_amount' => 60.0, 'interval' => 'day', 'weighting' => 'manual',
      'allocations' => weights.to_h { |sym, w| [assets.fetch(sym).id.to_s, w] }
    }.merge(settings))
    bot.set_missed_quote_amount
    bot.save!
    rows.each do |r|
      Transaction.insert!(r.except('asset').merge('bot_id' => bot.id, 'exchange_id' => alpaca.id, 'base_asset_id' => assets.fetch(r['asset']).id,
                                                  'quote_asset_id' => usd.id, 'base' => r['asset'], 'quote' => 'USD', 'side' => 0,
                                                  'transaction_type' => 'REGULAR', 'bot_interval' => 'day', 'bot_quote_amount' => 60,
                                                  'error_messages' => [], 'updated_at' => r['created_at']))
    end
    out = yield bot.reload, assets, alpaca
    raise ActiveRecord::Rollback
  end
  out
end
vector_row_id = 0
LEDGER_KINDS = %w[closed closed_nil_exec closed_zero_quote_exec open_partial open_unfilled unknown_market cancelled_partial abandoned failed skipped].freeze
# One REGULAR buy of `sym` in the state `kind`, priced near the member's usual price. Every decimal is a string, as the
# Rust test inserts it; Rails casts it to BigDecimal and binds it as a Float, as for any row.
ledger_row = lambda do |rng, sym, kind|
  vector_row_id += 1
  p = ({ 'VBTC' => 64_000, 'VETH' => 2500, 'VSOL' => 150 }.fetch(sym) * (0.9 + (rng.rand(20) / 100.0))).round(2).to_d
  q = (10 + rng.rand(200)).to_d
  a = (q / p).round(9)
  row = { 'asset' => sym, 'status' => 0, 'external_id' => "rust-vector-#{vector_row_id}", 'order_type' => 0, 'price' => p.to_s('F'),
          'created_at' => "2026-08-#{10 + (vector_row_id % 18)} 10:00:00" }
  case kind
  when 'closed' then row.merge('external_status' => 2, 'quote_amount' => q.to_s('F'), 'amount_exec' => a.to_s('F'), 'quote_amount_exec' => (a * p).to_s('F'))
  when 'closed_nil_exec' then row.merge('external_status' => 2, 'amount' => a.to_s('F'))
  when 'closed_zero_quote_exec' then row.merge('external_status' => 2, 'quote_amount' => q.to_s('F'), 'amount_exec' => a.to_s('F'), 'quote_amount_exec' => '0')
  when 'open_partial' then row.merge('external_status' => 1, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => (a / 3).round(9).to_s('F'),
                                     'quote_amount_exec' => ((a / 3).round(9) * p).to_s('F'))
  when 'open_unfilled' then row.merge('external_status' => 0, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => '0', 'quote_amount_exec' => '0')
  when 'unknown_market' then row.merge('external_status' => 0, 'quote_amount' => q.to_s('F'))
  when 'cancelled_partial' then row.merge('external_status' => 3, 'quote_amount' => q.to_s('F'), 'amount_exec' => (a / 2).round(9).to_s('F'),
                                          'quote_amount_exec' => ((a / 2).round(9) * p).to_s('F'))
  when 'abandoned' then row.merge('external_status' => 4, 'quote_amount' => q.to_s('F'))
  when 'failed' then row.merge('status' => 1, 'external_id' => nil, 'quote_amount' => q.to_s('F'), 'amount_exec' => '0', 'quote_amount_exec' => '0')
  when 'skipped' then row.merge('status' => 2, 'external_id' => nil, 'quote_amount' => q.to_s('F'), 'amount_exec' => '0', 'quote_amount_exec' => '0')
  when 'cancelled_limit_partial' then row.merge('external_status' => 3, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => (a / 2).round(9).to_s('F'),
                                                'quote_amount_exec' => ((a / 2).round(9) * p).to_s('F'))
  when 'abandoned_limit' then row.merge('external_status' => 4, 'order_type' => 1, 'amount' => a.to_s('F'))
  when 'failed_resting' then row.merge('status' => 1, 'external_status' => 1, 'order_type' => 1, 'amount' => a.to_s('F'), 'amount_exec' => (a / 3).round(9).to_s('F'),
                                       'quote_amount_exec' => ((a / 3).round(9) * p).to_s('F'))
  when 'skipped_closed' then row.merge('status' => 2, 'external_status' => 2, 'quote_amount' => q.to_s('F'), 'amount' => a.to_s('F'), 'amount_exec' => a.to_s('F'),
                                       'quote_amount_exec' => (a * p).to_s('F'))
  end
end
pairs_of = ->(weights) { BASKET_PAIRS.slice(*weights.keys) }
# Bot::Composition::Measurable#metrics' asset_breakdown amounts and #reserved_waiting_amounts(:buy), by member symbol.
ledger_rng = Random.new(2_202_610)
ledger_cases = Array.new(40) do
  members = %w[VBTC VETH VSOL].first(ledger_rng.rand(1..3))
  weights = members.size == 1 ? { members[0] => 1.0 } : Bots::DcaMultiAsset.new.send(:normalize_allocations, members.to_h { |m| [m, 1.0] })
  rows = members.flat_map { |sym| Array.new(ledger_rng.rand(0..4)) { ledger_row.(ledger_rng, sym, LEDGER_KINDS.sample(random: ledger_rng)) } }
  { weights:, rows: }
end
# Rows the random cases never hold, each one decisive for a filter: a cancelled or abandoned limit buy whose amount is
# set (a resting remainder only the external-status filter keeps out), and a failed or skipped row carrying amounts (kept
# out only by the submitted filter). Drawn after the random cases so those stay as they were.
filter_rng = Random.new(2_202_611)
FILTER_KINDS = %w[cancelled_limit_partial abandoned_limit failed_resting skipped_closed].freeze
ledger_cases += FILTER_KINDS.map { |kind| { weights: { 'VBTC' => 1.0 }, rows: [ledger_row.(filter_rng, 'VBTC', 'open_partial'), ledger_row.(filter_rng, 'VBTC', kind)] } }
ledger_cases << { weights: Bots::DcaMultiAsset.new.send(:normalize_allocations, { 'VBTC' => 1.0, 'VETH' => 1.0 }),
                  rows: FILTER_KINDS.flat_map { |kind| %w[VBTC VETH].map { |sym| ledger_row.(filter_rng, sym, kind) } } }
vectors['basket_ledgers'] = ledger_cases.map do |c|
  with_basket(c[:weights], rows: c[:rows]) do |bot, assets, _alpaca|
    m = bot.metrics(force: true)
    breakdown = m[:asset_breakdown] || {}
    symbol_of = ->(id) { assets.find { |_, a| a.id == id }&.first }
    { 'weights' => c[:weights], 'pairs' => pairs_of.(c[:weights]), 'rows' => c[:rows],
      'holdings' => assets.filter_map { |sym, a| (h = breakdown.dig(bot.key_for(a.id, m), :amount)) && [sym, h.to_d.to_s('F')] }.to_h,
      'reserved' => bot.send(:reserved_waiting_amounts, :buy).to_h { |id, amount| [symbol_of.(id), amount.to_d.to_s('F')] } }
  end
end

# The weights a basket is derived with: settings sliders (Floats), renormalised in Float with Ruby's compensated Array#sum,
# written into bot_index_assets.target_allocation decimal(10,6), and read back as BigDecimal, then #to_f.
bits = ->(f) { [f].pack('G').unpack1('H*') }
composition_rng = Random.new(2_202_640)
weight_sets = [{ 'VBTC' => 1.0 }, { 'VBTC' => 0.5, 'VETH' => 0.5 }, { 'VBTC' => 0.7, 'VETH' => 0.3 },
               { 'VBTC' => 0.334, 'VETH' => 0.333, 'VSOL' => 0.333 }, { 'VBTC' => 0.5, 'VETH' => 0.3, 'VSOL' => 0.2 },
               { 'VBTC' => 0.1, 'VETH' => 0.2, 'VSOL' => 0.3 }, { 'VBTC' => 0.6, 'VETH' => 0.4, 'VSOL' => 0.0 }] +
              Array.new(12) { Bots::DcaMultiAsset.new.send(:normalize_allocations, %w[VBTC VETH VSOL].to_h { |m| [m, composition_rng.rand(1..97).to_f] }) }
vectors['float_sum'] = (weight_sets.map(&:values) + [[0.1, 0.2, 0.3], [1.0e16, 1.0, -1.0e16], [0.333, 0.333, 0.334]])
                       .map { |values| [values.map(&bits), bits.(values.sum)] }
target_type = BotIndexAsset.type_for_attribute(:target_allocation)
derived = weight_sets.flat_map { |w| (t = w.values.sum).positive? ? w.values.map { |v| v / t } : [] }
vectors['decimal_10_6'] = (derived + [1.0 / 3, 2.0 / 3, 0.3333335, 0.1234565, 0.0000005, 0.9999995, 1.0e-7, 0.12345649999999999])
                          .map { |f| [bits.(f), target_type.cast(f).to_s('F')] }
composition_cases = [[weight_sets[0], []], [weight_sets[1], []], [weight_sets[1], %w[VETH]], [weight_sets[1], %w[VBTC VETH]],
                     [weight_sets[2], []], [weight_sets[2], %w[VBTC]], [weight_sets[3], []], [weight_sets[3], %w[VSOL]],
                     [weight_sets[4], []], [weight_sets[4], %w[VETH]], [weight_sets[5], []], [weight_sets[6], []]] +
                    weight_sets.drop(7).flat_map { |w| [[w, []], [w, [w.keys.sample(random: composition_rng)]]] }
# Members left whose stored decimal(10,6) targets do not sum to 1, so buyable_allocations re-weights them in BigDecimal: a
# four-asset basket with one member out (0.444444 + 0.333333 + 0.222222), and true thirds (0.333333 × 3). Then members that
# exited and trade again: the third refresh re-adds them (`readd`).
four = { 'VBTC' => 0.4, 'VETH' => 0.3, 'VSOL' => 0.2, 'VADA' => 0.1 }
thirds = { 'VBTC' => 1.0 / 3, 'VETH' => 1.0 / 3, 'VSOL' => 1.0 / 3 }
composition_cases = composition_cases.map { |w, u| [w, u, []] } +
                    [[four, %w[VADA], []], [four, %w[VETH], []], [thirds, [], []], [thirds, %w[VSOL], []],
                     [weight_sets[4], %w[VETH], %w[VETH]], [four, %w[VADA VSOL], %w[VSOL]], [thirds, %w[VBTC], %w[VBTC]]]
vectors['basket_compositions'] = composition_cases.map do |weights, untradable, readd|
  with_basket(weights) do |bot, assets, alpaca|
    failure = nil
    ticker_of = ->(sym) { Ticker.find_by!(exchange: alpaca, base_asset: assets.fetch(sym)) }
    entered = bot.bot_index_assets.to_h { |b| [b.asset_id, b.entered_at] }
    if untradable.any?
      untradable.each { |sym| ticker_of.(sym).update_columns(trading_enabled: false) }
      result = bot.refresh_composition
      failure = result.errors.to_sentence if result.failure?
    end
    if readd.any?
      readd.each { |sym| ticker_of.(sym).update_columns(trading_enabled: true) }
      bot.refresh_composition.then { |r| raise r.errors.to_sentence if r.failure? }
    end
    # Per row: entered_at as the first save wrote it, and exited_at blank.
    stamps = bot.bot_index_assets.reload.order(:id).map { |b| [assets.key(b.asset), b.entered_at == entered[b.asset_id], b.exited_at.nil?] }
    { 'weights' => weights, 'pairs' => pairs_of.(weights), 'exchange' => alpaca.name, 'untradable' => untradable, 'readd' => readd,
      'failure' => failure, 'stamps' => stamps,
      'index_rows' => bot.bot_index_assets.order(:id).map { |b| [assets.key(b.asset), b.target_allocation&.to_s('F'), b.in_index] },
      'members' => bot.send(:buyable_allocations).map { |a| [assets.key(a[:asset]), bits.(a[:target_allocation].to_f)] } }
  end
end

# Bot::Composition::OrderSetter#get_orders_data over recorded holdings and prices: every order it returns (base, price,
# base amount, quote amount) or its failure. Prices are read from the book below, not from the venue.
module VectorPrices
  mattr_accessor :book
  def get_ask_price(force: false) = Result::Success.new(VectorPrices.book.fetch([id, :ask]))
  def get_last_price(force: false) = Result::Success.new(VectorPrices.book.fetch([id, :last]))
end
Ticker.prepend(VectorPrices)
closed_row = lambda do |sym, value, price|
  vector_row_id += 1
  qty = (value.to_d / price.to_d).round(9)
  { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "rust-vector-#{vector_row_id}", 'order_type' => 0,
    'price' => price.to_s, 'quote_amount' => value.to_s, 'amount_exec' => qty.to_s('F'), 'quote_amount_exec' => (qty * price.to_d).to_s('F'),
    'created_at' => '2026-08-20 10:00:00' }
end
split_rng = Random.new(2_202_650)
base_prices = { 'VBTC' => 64_000.0, 'VETH' => 2500.0, 'VSOL' => 150.0 }
usual = { 'VBTC' => { 'ask' => '64000', 'last' => '63990' }, 'VETH' => { 'ask' => '2500', 'last' => '2499.5' }, 'VSOL' => { 'ask' => '150', 'last' => '149.9' } }
split_cases = Array.new(48) do
  members = %w[VBTC VETH VSOL].first(split_rng.rand(1..3))
  weights = members.size == 1 ? { members[0] => 1.0 } : Bots::DcaMultiAsset.new.send(:normalize_allocations, members.to_h { |m| [m, split_rng.rand(1..9).to_f] })
  rows = members.flat_map { |sym| Array.new(split_rng.rand(0..3)) { ledger_row.(split_rng, sym, LEDGER_KINDS.sample(random: split_rng)) } }
  prices = members.to_h { |m| p = base_prices[m] * (0.8 + (split_rng.rand(40) / 100.0)); [m, { 'ask' => format('%.3f', p), 'last' => format('%.3f', p * 0.999) }] }
  { weights:, rows:, prices:, limit: split_rng.rand < 0.5, x: %w[0.5 3 60 120 123.45 1000].sample(random: split_rng) }
end
split_cases += [
  # A limit price under the pair's precision: Rails fails before any order ("limit price rounds to zero at 3 decimals").
  { weights: { 'VBTC' => 0.5, 'VSOL' => 0.5 }, rows: [], prices: { 'VBTC' => usual['VBTC'], 'VSOL' => { 'ask' => '0.0004', 'last' => '0.0004' } }, limit: true, x: '60' },
  # At balance: each member holds its weight of 600 at the ask, so the contribution is spent by weight.
  { weights: { 'VBTC' => 0.334, 'VETH' => 0.333, 'VSOL' => 0.333 },
    rows: [closed_row.('VBTC', 200.4, 64_000), closed_row.('VETH', 199.8, 2500), closed_row.('VSOL', 199.8, 150)], prices: usual, limit: false, x: '60' },
  # Drifted past its share even after the contribution: that member's offset is zero, the other takes everything.
  { weights: { 'VBTC' => 0.5, 'VETH' => 0.5 }, rows: [closed_row.('VBTC', 1000, 64_000)], prices: usual.slice('VBTC', 'VETH'), limit: false, x: '60' },
  # One member: the offsets reduce to the contribution.
  { weights: { 'VBTC' => 1.0 }, rows: [closed_row.('VBTC', 300, 64_000)], prices: usual.slice('VBTC'), limit: true, x: '123.45' },
  # True thirds, stored as 0.333333 each and re-weighted in BigDecimal, valued at limit prices.
  { weights: { 'VBTC' => 1.0 / 3, 'VETH' => 1.0 / 3, 'VSOL' => 1.0 / 3 }, rows: [closed_row.('VBTC', 100, 64_000)], prices: usual, limit: true, x: '60' },
  # A resting limit buy counts as held: VETH's unfilled 0.08 at 2500 balances VBTC's 200, so the contribution splits by
  # weight instead of all going to VETH.
  { weights: { 'VBTC' => 0.5, 'VETH' => 0.5 },
    rows: [closed_row.('VBTC', 200, 64_000),
           { 'asset' => 'VETH', 'status' => 0, 'external_status' => 0, 'external_id' => "rust-vector-#{vector_row_id += 1}", 'order_type' => 1,
             'price' => '2500.0', 'amount' => '0.08', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-08-20 10:00:00' }],
    prices: usual.slice('VBTC', 'VETH'), limit: false, x: '60' },
  # 7:7:1 from nothing, stored as 0.466667, 0.466667 and 0.066667 and re-weighted: the offsets sum past the contribution,
  # and the last leg is capped by what is left rather than by its own share.
  { weights: { 'VBTC' => 7.0 / 15, 'VETH' => 7.0 / 15, 'VSOL' => 1.0 / 15 }, rows: [], prices: usual, limit: false, x: '60' }
]
vectors['basket_splits'] = split_cases.map do |c|
  settings = c[:limit] ? { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } : {}
  with_basket(c[:weights], settings:, rows: c[:rows]) do |bot, assets, alpaca|
    VectorPrices.book = c[:prices].each_with_object({}) do |(sym, p), book|
      t = Ticker.find_by!(exchange: alpaca, base_asset: assets.fetch(sym))
      book[[t.id, :ask]] = BigDecimal(p['ask'])
      book[[t.id, :last]] = BigDecimal(p['last'])
    end
    r = bot.send(:get_orders_data, BigDecimal(c[:x]))
    { 'weights' => c[:weights], 'pairs' => pairs_of.(c[:weights]), 'rows' => c[:rows], 'prices' => c[:prices], 'limit' => c[:limit], 'x' => c[:x],
      'orders' => r.success? ? r.data.map { |o| [o[:ticker].base, o[:price].to_d.to_s('F'), o[:amount].to_d.to_s('F'), o[:quote_amount].to_d.to_s('F')] } : nil,
      'failure' => r.failure? ? r.errors.to_sentence : nil }
  end
end

# Bot::QuoteAmountLimitable#quote_amount_available_before_limit_reached and
# #quote_amount_limit_reached? as Ruby computes them. The closed and waiting buckets pluck decimal columns (BigDecimal); the
# stopped bucket plucks Arel.sql('COALESCE(quote_amount_exec, 0)'), which SQLite answers as its own INTEGER or REAL (Integer
# or Float in Ruby); each bucket is summed by Array#sum, the buckets added in that order, and the limit is the settings
# JSON's Integer or Float. Recorded with the class of the result: a Float leaks into the remainder (cap 60.03, a sole
# cancelled fill of 60.02: 0.00999999999999801, under the 0.01 floor).
cap_row = lambda do |kind, q|
  vector_row_id += 1
  row = { 'asset' => 'VBTC', 'status' => 0, 'external_id' => "rust-vector-#{vector_row_id}", 'order_type' => 0, 'price' => '64000',
          'created_at' => '2026-08-20 10:00:00' }
  case kind
  when 'closed' then row.merge('external_status' => 2, 'quote_amount' => q, 'quote_amount_exec' => q, 'amount_exec' => '0.001')
  when 'unknown' then row.merge('external_status' => 0, 'quote_amount' => q)
  when 'cancelled' then row.merge('external_status' => 3, 'quote_amount' => '100', 'quote_amount_exec' => q, 'amount_exec' => '0.001')
  when 'abandoned' then row.merge('external_status' => 4, 'quote_amount' => '100')
  end
end
cap_fixed = [
  [60.03, [%w[cancelled 60.02]]],                        # a Float remainder under the floor: reached
  [60.03, [%w[closed 60.02]]],                           # the same in BigDecimal: exactly 0.01, not reached
  [60.03, [%w[closed 30.01], %w[cancelled 30.01]]],      # BigDecimal + Float
  [60.03, [%w[cancelled 30.01], %w[cancelled 30.01]]],   # two Floats, Kahan-summed
  [100, [%w[cancelled 99.99]]],                          # Integer - Float
  [100, [%w[cancelled 99.995]]],
  [100, [%w[closed 99.995]]],
  [60.03, [['abandoned', nil], %w[cancelled 60.02]]],    # Integer 0, then a Float
  [60.03, [%w[cancelled 60], %w[cancelled 0.02]]],       # an integral fill is stored INTEGER, then a Float
  [1000, []],
  [50.5, [%w[unknown 25.25], %w[cancelled 25.24]]],
  [0.3, [%w[cancelled 0.1], %w[cancelled 0.2]]],
  [0.31, [%w[cancelled 0.1], %w[cancelled 0.2]]],
  [120.07, [%w[closed 40.02], %w[unknown 40.02], %w[cancelled 40.02]]]
]
cap_rng = Random.new(2_202_670)
cap_random = Array.new(26) do
  cap = [60.03, 100, 99.99, 120.07, 0.3, 75.5].sample(random: cap_rng)
  rows = Array.new(cap_rng.rand(1..4)) do
    kind = %w[closed unknown cancelled cancelled abandoned].sample(random: cap_rng)
    [kind, kind == 'abandoned' ? nil : format('%.2f', cap_rng.rand(1..3000) / 100.0)]
  end
  [cap, rows]
end
vectors['amount_caps'] = (cap_fixed + cap_random).map do |cap, specs|
  rows = specs.map { |kind, q| cap_row.(kind, q) }
  with_basket({ 'VBTC' => 1.0 }, settings: { 'quote_amount_limited' => true, 'quote_amount_limit' => cap }, rows:) do |bot, _assets, _alpaca|
    bot.update_columns(transient_data: bot.transient_data.merge('quote_amount_limit_enabled_at' => '2026-08-01T00:00:00.000Z'))
    bot = Bot.find(bot.id)
    left = bot.quote_amount_available_before_limit_reached
    value = case left
            when Float then { 'class' => 'Float', 'f' => [left].pack('G').unpack1('H*') }
            when BigDecimal then { 'class' => 'BigDecimal', 'd' => left.to_s('F') }
            else { 'class' => left.class.name, 'i' => left.to_s }
            end
    { 'weights' => { 'VBTC' => 1.0 }, 'pairs' => pairs_of.({ 'VBTC' => 1.0 }), 'rows' => rows, 'cap' => cap, 'available' => value,
      'reached' => bot.quote_amount_limit_reached? }
  end
end

# Bot::Lifecycle#start(start_fresh: false) on a stopped one-asset basket (daily, 60, started 2026-09-01 10:00), called at
# 2026-09-03 15:00 UTC: whether Rails runs it now, at the next checkpoint or at a delayed first run, read from the
# Bot::ActionJob it enqueues. The engine makes the same decision when the web asks it to continue a bot.
continue_row = lambda do |n, quote_exec, created_at|
  { 'asset' => 'VBTC', 'status' => 0, 'side' => 0, 'transaction_type' => 'REGULAR', 'external_id' => "rust-continue-#{n}", 'order_type' => 0,
    'price' => '64000', 'external_status' => 2, 'quote_amount' => '60', 'quote_amount_exec' => quote_exec, 'amount_exec' => '0.0009375',
    'created_at' => created_at }
end
stamped = '2026-09-03T10:00:00.500Z' # today's tick stamped last_action_job_at
two_days = [['60', '2026-09-01 10:00:01'], ['60', '2026-09-02 10:00:01']]
continue_cases = [
  ['owes_its_contribution', stamped, two_days, {}],                         # stamped, never placed: 60 owed, not under 60
  ['nothing_owed', stamped, two_days + [['60', '2026-09-03 10:00:01']], {}],
  ['a_cent_short_of_a_contribution', stamped, two_days + [['0.01', '2026-09-03 10:00:01']], {}],
  ['never_ticked', nil, [], {}],                                            # not restarting
  ['the_cap_leaves_less_than_a_contribution', stamped, [['60', '2026-09-01 10:00:01']],
   { 'quote_amount_limited' => true, 'quote_amount_limit' => 100 }],        # 120 owed, 40 left under the cap
  ['a_smart_interval_compares_with_its_split_amount', stamped, two_days,
   { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 }], # 7 eight-hour intervals: 20 owed, not under 20
  ['a_future_start_time_is_not_a_delayed_first_run', stamped, two_days,
   { 'start_time_enabled' => true, 'start_time_mode' => 'date', 'start_at' => '2026-10-01T00:00:00Z' }]
]
adapter = ActiveJob::Base.queue_adapter
ActiveJob::Base.queue_adapter = :test
vectors['continue_start'] = continue_cases.each_with_index.map do |(name, last_action_job_at, specs, settings), i|
  rows = specs.each_with_index.map { |(q, t), n| continue_row.((i * 10) + n, q, t) }
  with_basket({ 'VBTC' => 1.0 }, settings:, rows:) do |bot, _assets, _alpaca|
    transient = bot.transient_data.merge('last_action_job_at' => last_action_job_at, 'quote_amount_limit_enabled_at' => '2026-08-01T00:00:00.000Z').compact
    bot.update_columns(status: Bot.statuses[:stopped], started_at: Time.utc(2026, 9, 1, 10), settings_changed_at: nil, transient_data: transient)
    bot = Bot.find(bot.id)
    now = Time.utc(2026, 9, 3, 15)
    travel_to(now, with_usec: true) do
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      raise "#{name}: Rails refused the start: #{bot.errors.full_messages}" unless bot.start(start_fresh: false)

      jobs = ActiveJob::Base.queue_adapter.enqueued_jobs.select { |j| j[:job] == Bot::ActionJob }
      raise "#{name}: #{jobs.size} action jobs" unless jobs.size == 1

      at = jobs.first[:at] && Time.zone.at(jobs.first[:at]).utc
      decision = if at.nil? then 'now'
                 elsif at.round(6) == bot.next_interval_checkpoint_at.round(6) then 'checkpoint'
                 else "at #{at.iso8601(6)}"
                 end
      { 'name' => name, 'settings' => settings, 'last_action_job_at' => last_action_job_at, 'rows' => rows, 'now' => now.iso8601(6),
        'decision' => decision }
    end
  end
end
ActiveJob::Base.queue_adapter = adapter

# The web UI (rust/src/web). Everything below is what Rails itself answers, so the Rust port is held to it.
helpers = ApplicationController.helpers
include ActiveSupport::Testing::TimeHelpers
shown = ->(value) { ERB::Util.html_escape(value).to_s } # what a view prints: escaped unless html_safe
i18n_calls = [
  ['en', 'devise.sessions.new.title', {}], ['de', 'devise.sessions.new.title', {}],
  ['en', 'links.api', {}], ['de', 'links.api', {}], # English only: the German page falls back; `&` is escaped
  ['en', 'bot.add_api_keys', { 'exchange' => %q(A<b>&"') }], # a plain key: the whole text is escaped
  ['en', 'ads.dca_profit_html', { 'years' => '<4>', 'profit' => '12', 'sp500_diff' => 'a&b' }], # an HTML key: only the arguments are
  ['en', 'devise.failure.invalid', { 'authentication_keys' => 'email' }],
  ['en', 'devise.failure.locked', {}], ['de', 'devise.failure.locked', {}], # from the Devise gem, English only
  ['en', 'devise.sessions.two_factor.title', {}], ['de', 'devise.sessions.two_factor.title', {}], # missing everywhere
  ['en', 'nowhere.some_key_html', {}], ['en', 'nowhere.user_id', { 'name' => 'a<b' }], ['en', 'nowhere._odd__key', { 'count' => 3 }]
] + %w[en pl ru].product([0, 1, 2, 4, 5, 11, 12, 21, 22, 24, 25, 101, 112]).flat_map { |locale, n| %w[days_left errors.messages.too_short].map { |key| [locale, key, { 'count' => n }] } }
vectors['i18n'] = {
  'locales' => I18n.available_locales.map(&:to_s),
  'default' => I18n.default_locale.to_s,
  'calls' => i18n_calls.map do |locale, key, args|
    I18n.with_locale(locale) do
      { 'locale' => locale, 'key' => key, 'args' => args,
        'view' => shown.(helpers.t(key, **args.symbolize_keys)), 'text' => I18n.t(key, **args.symbolize_keys) }
    end
  end,
  'escape' => [%q(a<b>&"'c), 'plain', 'ż & ☃'].map { |s| [s, shown.(s)] }
}
ts = helpers.turbo_stream
vectors['turbo'] = {
  'replace' => ts.replace('bot_1', '<p>a &amp; b</p>'.html_safe), 'update' => ts.update('bot_1', '<p>x</p>'.html_safe),
  'append' => ts.append('orders', '<tr></tr>'.html_safe), 'prepend' => ts.prepend('flash', '<div>hi</div>'.html_safe),
  'remove' => ts.remove('bot_1'), 'refresh' => ts.refresh(request_id: nil),
  'redirect' => ts.action(:redirect, '/de/bots?a=1&b=2'), # SharedHelper#turbo_stream_redirect
  # Bot#broadcast_columns_lock_update, as Turbo::StreamsChannel.broadcast_action_to renders it.
  'add_class' => helpers.turbo_stream_action_tag(:add_class, target: 'columns_bot_1', template: nil, 'class-name': 'bot-locked'),
  'remove_class' => helpers.turbo_stream_action_tag(:remove_class, target: 'columns_bot_1', template: nil, 'class-name': 'bot-locked'),
  'stream_from' => helpers.turbo_stream_from('user_7', :bot_updates),
  'stream_name' => Turbo::StreamsChannel.verified_stream_name(Turbo::StreamsChannel.signed_stream_name(['user_7', :bot_updates])),
  'content_type' => Mime[:turbo_stream].to_s
}
remote_ip = lambda do |remote_addr, forwarded_for, client_ip|
  env = { 'REMOTE_ADDR' => remote_addr, 'HTTP_X_FORWARDED_FOR' => forwarded_for, 'HTTP_CLIENT_IP' => client_ip }.compact
  ActionDispatch::RemoteIp::GetIp.new(ActionDispatch::Request.new(env), false, ActionDispatch::RemoteIp::TRUSTED_PROXIES).to_s
end
trusted_proxy = ->(addr) { ActionDispatch::RemoteIp::TRUSTED_PROXIES.any? { |proxy| proxy === addr } }
vectors['remote_ip'] = [
  # a peer that is a trusted proxy: one hop, a private hop behind it, several hops, nothing but private hops
  ['10.0.0.5', '198.51.100.7', nil], ['10.0.0.5', '198.51.100.7, 10.0.0.9', nil], ['10.0.0.5', '1.1.1.1, 198.51.100.7', nil],
  ['10.0.0.5', '203.0.113.50, 198.51.100.7, 10.0.0.9, 192.168.1.4', nil], ['10.0.0.5', '10.1.1.1, 192.168.1.1', nil],
  ['172.16.0.1', '172.32.0.1', nil], ['169.254.1.1', 'fe80::1', nil],
  # malformed entries
  ['10.0.0.5', 'not-an-ip, 198.51.100.7', nil], ['10.0.0.5', '198.51.100.7, unknown, , 10.0.0.9', nil], ['10.0.0.5', '198.51.100.7/8', nil],
  ['10.0.0.5', '', nil],
  # entries with a port, as a proxy may write its peer, also behind an entry only the caller wrote
  ['10.0.0.5', '203.0.113.7:54321', nil], ['10.0.0.5', '[2001:db8::7]:54321', nil], ['10.0.0.5', '[2001:db8::7]', nil],
  ['10.0.0.5', '198.51.100.99, 203.0.113.7:54321', nil], ['10.0.0.5', '198.51.100.99, [2001:db8::7]:54321', nil],
  ['10.0.0.5', '198.51.100.99, 203.0.113.7:54321, 10.0.0.9:443', nil], ['10.0.0.5', '198.51.100.99 203.0.113.7', nil],
  # entries Rails cannot read, behind an entry only the caller wrote; IPv4-mapped addresses
  ['10.0.0.5', '198.51.100.99, garbage, 10.0.0.9', nil], ['10.0.0.5', '198.51.100.99, 203.0.113.7:notaport', nil],
  ['10.0.0.5', '198.51.100.99, 203.0.113.7/32', nil], ['10.0.0.5', '198.51.100.99, ::ffff:10.0.0.9', nil],
  ['10.0.0.5', '198.51.100.99, ::ffff:203.0.113.7', nil], ['10.0.0.5', '198.51.100.99, [::ffff:203.0.113.7]:443', nil],
  # Client-Ip
  ['127.0.0.1', nil, '198.51.100.8'], ['127.0.0.1', '198.51.100.7', '198.51.100.8'], ['10.0.0.5', nil, '198.51.100.8:1234'],
  ['10.0.0.5', nil, '198.51.100.9, 198.51.100.8'],
  # IPv6
  ['10.0.0.5', '2001:db8::1', nil], ['::1', 'fd00::1, 2001:db8::2', nil], ['fd00::5', '2001:db8::7', nil],
  # a peer that is not a trusted proxy, with and without headers of its own making
  ['203.0.113.9', nil, nil], ['203.0.113.9', '198.51.100.7', nil], ['203.0.113.9', '198.51.100.7, 10.0.0.9', nil],
  ['203.0.113.9', nil, '198.51.100.8'], ['2001:db8::9', '198.51.100.7', nil]
].map do |addr, forwarded, client|
  { 'remote_addr' => addr, 'forwarded_for' => forwarded, 'client_ip' => client, 'peer_trusted' => trusted_proxy.(addr),
    'ip' => remote_ip.(addr, forwarded, client) }
end
vectors['tracker'] = {
  'cash' => (Tracker::UnfundedCash::FIAT + Tracker::UnfundedCash::STABLECOINS).sort,
  # User#show_cash? for what the tracker_settings column can hold.
  'show_cash' => [nil, {}, { 'other' => true }, { 'show_cash' => true }, { 'show_cash' => false }, { 'show_cash' => nil },
                  { 'show_cash' => '' }, { 'show_cash' => ' ' }, { 'show_cash' => 'false' }, { 'show_cash' => 0 }, { 'show_cash' => [] },
                  { 'show_cash' => [1] }, { 'show_cash' => {} }].map do |settings|
    { 'column' => settings&.to_json, 'shown' => User.new(tracker_settings: settings).show_cash? }
  end
}
vectors['navbar'] = {
  'bot_count' => [0, 1, 9, 10, 99, 100, 999, 1000, 12_345].map do |count|
    size = helpers.send(:bot_count_font_size, count)
    { 'count' => count, 'font_size' => size.to_s, 'baseline' => helpers.send(:bot_count_baseline, size).to_s }
  end
}
vectors['rack_attack'] = {
  'throttles' => Rack::Attack.throttles.slice('users/login', 'users/verify_two_factor').transform_values { |t| { 'limit' => t.limit, 'period' => t.period } },
  'normalize' => ['/login', '/login/', '//login', '/de//login/', '/'].map { |p| [p, RackAttackPaths.normalize(p)] },
  'body' => "#{I18n.t('errors.throttled')}\n"
}
vectors['devise'] = { 'maximum_attempts' => Devise.maximum_attempts, 'unlock_in' => Devise.unlock_in.to_i,
                      'pending_ttl' => Users::SessionsController::PENDING_TTL.to_i, 'session_expire_after' => Rails.application.config.session_options[:expire_after].to_i }
# request.base_url, which the Origin header of a form POST must equal (valid_request_origin?), from the
# stack production runs: Puma builds the env (it derives rack.url_scheme from the forwarded headers),
# ActionDispatch::AssumeSSL sits in front of the app when config/environments/production.rb turns SSL
# on ('ssl' below: it sets assume_ssl and force_ssl from the one flag), and Rack and Action Dispatch
# read the result. Each request goes over a raw socket, so header lines arrive as written, repeats included.
require 'puma'
require 'puma/server'
require 'socket'
unless Rails.root.join('config/environments/production.rb').read.match?(/config\.assume_ssl = ssl_enabled\n\s*config\.force_ssl = ssl_enabled\n/)
  raise 'production.rb no longer sets assume_ssl and force_ssl from the one flag, which the base_url vectors assume'
end
base_url_app = ->(env) { [200, { 'content-type' => 'text/plain' }, [ActionDispatch::Request.new(env).base_url]] }
# [server, port] of `app` behind Puma on a local port.
over_puma = lambda do |app|
  server = Puma::Server.new(app, nil, log_writer: Puma::LogWriter.null)
  port = server.add_tcp_listener('127.0.0.1', 0).addr[1]
  server.run
  [server, port]
end
# The body of the answer to one GET with exactly these header lines.
ask_puma = lambda do |port, host, headers|
  answer = TCPSocket.open('127.0.0.1', port) do |socket|
    socket.write("GET / HTTP/1.1\r\nHost: #{host}\r\n#{headers.map { |line| "#{line}\r\n" }.join}Connection: close\r\n\r\n")
    socket.read
  end
  head, body = answer.split("\r\n\r\n", 2)
  raise "#{host} #{headers}: #{head}" unless head.start_with?('HTTP/1.1 200')

  body
end
base_url_servers = { false => base_url_app, true => ActionDispatch::AssumeSSL.new(base_url_app) }.transform_values(&over_puma)
base_url = ->(ssl, host, headers) { ask_puma.(base_url_servers.fetch(ssl).last, host, headers) }
forwarded_headers = [
  [], ['X-Forwarded-Proto: https'], ['X-Forwarded-Proto: http'], ['X-Forwarded-Proto: https,http'], ['X-Forwarded-Proto: http,https'],
  ['X-Forwarded-Proto: https, http'], ['X-Forwarded-Proto: http https'], ['X-Forwarded-Proto: https', 'X-Forwarded-Proto: http'],
  ['X-Forwarded-Proto: HTTPS'], ['X-Forwarded-Proto: ftp'], ['X-Forwarded-Proto: https,ftp'], ['X-Forwarded-Proto: httpsx'],
  ['X-Forwarded-Proto: wss'], ['X-Forwarded-Proto: ws'],
  ['X-Forwarded-Ssl: on'], ['X-Forwarded-Ssl: off'], ['X-Forwarded-Ssl: On'], ['X-Forwarded-Ssl: on', 'X-Forwarded-Proto: http'],
  ['X-Forwarded-Scheme: https'], ['X-Forwarded-Scheme: http'], ['X-Forwarded-Proto: http', 'X-Forwarded-Scheme: https'],
  ['X-Forwarded-Proto: ftp', 'X-Forwarded-Scheme: https'], ['X-Forwarded-Proto: https', 'X-Forwarded-Scheme: http'],
  ['X-Forwarded-Proto: ftp', 'X-Forwarded-Scheme: HTTPS'],
  ['Forwarded: proto=https'], ['Forwarded: proto=http', 'X-Forwarded-Proto: https'], ['Forwarded: proto=https', 'X-Forwarded-Proto: http'],
  ['Forwarded: for=192.0.2.1;proto=https, for=198.51.100.2;proto=http'], ['Forwarded: for=192.0.2.1;proto=http, for=198.51.100.2;proto=https'],
  ['Forwarded: Proto = "https"'], ['Forwarded: for="[2001:db8::1]:4711";proto=https;by=203.0.113.43'], ['Forwarded: proto="ht\\tps" ; for=x'],
  ['Forwarded: proto=https;secret=1'], ['Forwarded: proto=ftp', 'X-Forwarded-Proto: https'], ['Forwarded: for=192.0.2.1', 'X-Forwarded-Proto: https'],
  ['Forwarded: proto=http', 'X-Forwarded-Ssl: on'], ['Forwarded: proto="https'], ['Forwarded: proto=https', 'Forwarded: proto=http'],
  ['X-Forwarded-Host: public.example.org'], ['X-Forwarded-Host: public.example.org', 'X-Forwarded-Proto: https'],
  ['X-Forwarded-Host: public.example.org:8443', 'X-Forwarded-Proto: https'], ['X-Forwarded-Host: public.example.org:443', 'X-Forwarded-Proto: https'],
  ['X-Forwarded-Host: first.example, public.example.org'], ['X-Forwarded-Host: first.example,public.example.org:81'],
  ['X-Forwarded-Host: public.example.org,'], ['X-Forwarded-Host:'],
  ['X-Forwarded-Port: 8443', 'X-Forwarded-Proto: https'], ['Forwarded: host=public.example.org;proto=https'],
  # a header sent as several lines: Puma hands Rack one value
  ['X-Forwarded-Proto: http', 'X-Forwarded-Proto: https'], ['X-Forwarded-Scheme: https', 'X-Forwarded-Scheme: http'],
  ['X-Forwarded-Ssl: on', 'X-Forwarded-Ssl: on'], ['X-Forwarded-Host: first.example', 'X-Forwarded-Host: public.example.org'],
  ['X-Forwarded-Host: public.example.org', 'X-Forwarded-Proto: https', 'X-Forwarded-Host: last.example:8443']
]
base_url_cases = forwarded_headers.map { |headers| ['bot.example.com:8080', headers] } +
                 ['bot.example.com', 'bot.example.com:443', 'bot.example.com:80', '[::1]:3000', '[::1]'].product([[], ['X-Forwarded-Proto: https']])
vectors['base_url'] = [false, true].product(base_url_cases).map do |ssl, (host, headers)|
  { 'ssl' => ssl, 'host' => host, 'headers' => headers, 'base_url' => base_url.(ssl, host, headers) }
end
base_url_servers.each_value { |server, _| server.stop(true) }
# The client address when a forwarding header arrives as several lines (a proxy that appends its
# own line after whatever the caller sent): what Puma makes of the lines, and the address Rails
# then takes. The peer is 127.0.0.1, a trusted proxy.
remote_ip_server = over_puma.(lambda do |env|
  ip = ActionDispatch::RemoteIp::GetIp.new(ActionDispatch::Request.new(env), false, ActionDispatch::RemoteIp::TRUSTED_PROXIES).to_s
  [200, { 'content-type' => 'application/json' }, [{ 'remote_addr' => env['REMOTE_ADDR'], 'forwarded_for' => env['HTTP_X_FORWARDED_FOR'],
                                                     'client_ip' => env['HTTP_CLIENT_IP'], 'ip' => ip }.to_json]]
end)
vectors['remote_ip_lines'] = [
  ['X-Forwarded-For: 198.51.100.7'], ['X-Forwarded-For: 1.1.1.1', 'X-Forwarded-For: 198.51.100.7'],
  ['X-Forwarded-For: 1.1.1.1, 2.2.2.2', 'X-Forwarded-For: 198.51.100.7, 10.0.0.9'],
  ['X-Forwarded-For: 198.51.100.7', 'X-Forwarded-For: 10.0.0.9', 'X-Forwarded-For: 192.168.1.4'],
  ['X-Forwarded-For: 198.51.100.7', 'X-Forwarded-For: 1.1.1.1'],
  ['Client-Ip: 1.1.1.1', 'Client-Ip: 198.51.100.8'],
  ['X-Forwarded-For: 1.1.1.1', 'Client-Ip: 198.51.100.8', 'X-Forwarded-For: 198.51.100.7']
].map { |headers| { 'headers' => headers }.merge(JSON.parse(ask_puma.(remote_ip_server.last, 'bot.example.com', headers))) }
remote_ip_server.first.stop(true)
# ActionDispatch::HostAuthorization as production runs it when ALLOWED_HOSTS is set (rust/src/web/mod.rs:
# `allowed_hosts`, `host_allowed`, `Config::blocked_hosts`, `blocked_host`). `config.hosts` is built by
# production.rb's own lines, run here on a stand-in for `config`; what a host is allowed is asked of
# Action Pack's matcher; and the answers come from the middleware behind Puma, as the base_url vectors do.
production_rb = Rails.root.join('config/environments/production.rb').read
hosts_lines = production_rb[/^  if ENV\['ALLOWED_HOSTS'\]\.present\?\n.*?^  end\n/m] or
  raise 'production.rb no longer builds config.hosts from ALLOWED_HOSTS, which the host vectors assume'
raise 'production.rb sets config.host_authorization, which the host vectors assume it does not' if production_rb.include?('host_authorization')
config_hosts = lambda do |value|
  config = Struct.new(:hosts).new([]) # Rails' own default outside development
  kept = ENV.fetch('ALLOWED_HOSTS', nil)
  ENV['ALLOWED_HOSTS'] = value
  begin
    binding.eval(hosts_lines) # rubocop:disable Security/Eval
    config.hosts
  ensure
    ENV['ALLOWED_HOSTS'] = kept
  end
end
host_entries = ['app.example', '.apps.example', 'app.example:8443', '.apps.example:8443', '127.0.0.1', 'localhost', '[::1]', '::1', '', '.', 'a+b.example', 'App.Example']
host_values = ['app.example', 'APP.EXAMPLE', 'app.example:80', 'app.example:8443', 'app.example:', 'app.example:80:90', 'app.example:8443:1', 'app.example.', 'xapp.example',
               'app.examplex', 'app-example', 'apps.example', 'bot.apps.example', 'BOT.Apps.Example:3000', 'a.b.apps.example', '.apps.example', 'bot_x.apps.example', 'bot-1.apps.example',
               'bot.apps.example:8443', 'evil.example', 'evilapps.example', '127.0.0.1', '127.0.0.1:3000', '127.0.0.2', 'localhost', 'localhost:3000', 'localhost.evil.example',
               '[::1]', '[::1]:3000', '::1', '', ':80', 'x.', 'x.:80', 'a+b.example', 'aab.example', 'app.example, evil.example', "app.example\t", ' app.example']
host_requests = [
  ['app.example, .apps.example', 'app.example', [], false], ['app.example, .apps.example', 'evil.example', [], false], ['app.example, .apps.example', 'evil.example', [], true],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: evil.example'], false], ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: bot.apps.example'], false],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: evil.example, app.example'], false],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: app.example, evil.example'], false],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: app.example', 'X-Forwarded-Host: evil.example'], false],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: evil.example', 'X-Forwarded-Host: app.example'], false],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: evil.example,'], false], ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: app.example,  evil.example'], false],
  ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host:  '], false], ['app.example, .apps.example', 'app.example', ['X-Forwarded-Host: ,'], false],
  ['app.example, .apps.example', 'evil.example', ['X-Forwarded-Host: app.example'], false], ['app.example, .apps.example', 'evil.example', ['X-Forwarded-Host: other.example'], false],
  ['app.example, .apps.example', 'localhost:3000', [], false], ['app.example, .apps.example', '127.0.0.1', [], false],
  ['app.example, .apps.example', 'app.example', ['X-Requested-With: xmlhttprequest', 'X-Forwarded-Host: evil.example'], false],
  [nil, 'evil.example', ['X-Forwarded-Host: other.example'], false], [' ', 'evil.example', [], false], [',', 'evil.example', [], false], [',', 'localhost', [], false]
]
host_ask = lambda do |port, host, headers, xhr|
  lines = headers + (xhr ? ['X-Requested-With: XMLHttpRequest'] : [])
  answer = TCPSocket.open('127.0.0.1', port) do |socket|
    socket.write("GET / HTTP/1.1\r\nHost: #{host}\r\n#{lines.map { |line| "#{line}\r\n" }.join}Connection: close\r\n\r\n")
    socket.read
  end
  head, body = answer.split("\r\n\r\n", 2)
  { 'status' => head[%r{\AHTTP/1.1 (\d+)}, 1].to_i, 'content_type' => head[/^content-type: (.*?)\r?$/i, 1], 'body' => body,
    'other_headers' => head.lines.drop(1).map { |line| line[/\A[^:]+/].downcase }.reject { |name| %w[content-type content-length connection].include?(name) } }
end
host_servers = Hash.new do |servers, hosts|
  inner = ->(_env) { [200, { 'content-type' => 'text/plain' }, ['passed']] }
  # Rails leaves the middleware out when config.hosts is empty (DefaultMiddlewareStack).
  servers[hosts] = over_puma.(hosts.empty? ? inner : ActionDispatch::HostAuthorization.new(inner, hosts))
end
vectors['host_authorization'] = {
  'hosts' => [nil, '', ' ', ',', 'app.example', ' app.example , .apps.example ', 'a.example,,b.example', 'a.example, ', 'a.example,', ',a.example', "a.example\t,\nb.example:8443"]
    .map { |value| { 'allowed_hosts' => value, 'hosts' => config_hosts.(value) } },
  'allows' => host_entries.product(host_values).map do |entry, host|
    { 'entry' => entry, 'host' => host, 'allowed' => ActionDispatch::HostAuthorization::Permissions.new([entry]).allows?(host) }
  end,
  'requests' => host_requests.map do |allowed, host, headers, xhr|
    { 'allowed_hosts' => allowed, 'host' => host, 'headers' => headers, 'xhr' => xhr }.merge(host_ask.(host_servers[config_hosts.(allowed)].last, host, headers, xhr))
  end
}
host_servers.each_value { |server, _| server.stop(true) }
# What the OAuth provider's pure rules must reproduce (rust/src/web/oauth.rs), each asked of the code Rails runs:
# Ruby's URI.parse and URI#to_s, Doorkeeper's redirect-URI validator, its URIChecker and URIBuilder, Base64.decode64
# and Doorkeeper's reading of an `Authorization: Basic` header.
oauth_uris = ['https://client.example/callback', 'HTTPS://Client.Example:0443/Callback?x=1', 'http://client.example:80/cb', 'http://client.example:080/cb', 'http://client.example:/cb',
              'http://client.example:8080/cb', 'https://client.example:80/cb', 'http://client.example', 'http://u:p@localhost:3000/cb?x=1#f', 'http://[::1]:9/cb', 'http://[::1]/cb',
              'http://LOCALHOST/cb', 'https://c.example/cb?x=hello world', "https://c.example/cb?x=a\tb\r\nc", 'http://127.0.0.1/cb?a[]=1&b={}|^`"<>\\', "http://localhost/cb?\u0000",
              "http://localhost/cb?x=\u007F\u0001", "http://localhost/cb?x=a?b/c:d@e'f(g)h*i+j,k;l=m!n$o&p", 'http://localhost/cb?x=%7e~', 'http://localhost/cb?x=%zz',
              "http://localhost/cb?x=%z\tz", 'http://localhost/cb?x=%z', 'http://localhost/cb?x=%', 'http://localhost/cb?x=%a', 'http://localhost/cb?%zzz', 'http://localhost/cb?',
              'http://localhost/cb?#', 'http://localhost/cb#', 'http://127.0.0.1/c b', 'http://127.0.0.1/cb#a b', "http://localhost\t/cb", "\thttp://localhost/cb", "http://localhost/cb\n",
              'http://localhost/cb?x=é', 'http://localhost:99999999999999999999/cb', 'http://localhost:0/cb', 'http://localhost:00/cb', 'myapp://callback', 'myapp://callback:9/x',
              'myapp://callback:80', 'ws://h:80/x', 'wss://h:443/x', 'ws://h:443/x', '//host/p', '//host:8/p?q', '/callback', 'callback', '', 'world', 'urn:ietf:wg:oauth:2.0:oob',
              'javascript:alert(1)', 'localhost:3000/cb', 'mailto:a@b', 'https:///callback', 'http://', 'http://@h/x', 'http://u@/x', 'http://h/%zz', 'http://h/a%20b', 'a:b', 'a:/b']
oauth_uri_parts = lambda do |text|
  uri = URI.parse(text)
  { 'scheme' => uri.scheme, 'userinfo' => uri.userinfo, 'host' => uri.host, 'opaque' => !uri.opaque.nil?, 'fragment' => uri.fragment }
    .merge(uri.opaque ? {} : { 'path' => uri.path, 'query' => uri.query, 'to_s' => uri.to_s })
rescue URI::InvalidURIError
  nil
end
oauth_registered = ['https://client.example/callback', "https://c.example/cb#a\nhttps://c.example/cb#b", 'https://c.example/cb#', 'https://c.example/cb?x=hello world',
                    "https://c.example/cb?x=a\tb", "https://c.example/cb?x=a\vb", "https://c.example/cb?x=a\fb", "https://c.example/cb?x=a\rb", 'https://c.example/cb?x=1 https://b.example/cb',
                    'https://c.example/cb?x=1 javascript:alert(1)', 'https://c.example/cb?x=1 VBScript:x', 'https://c.example/cb?x=1 data:text/html,x', 'https://c.example/cb?x=1 urn:x',
                    'https://c.example/cb?x=1 localhost:3000/cb', 'https://c.example/cb?x=1 localhost://h/cb', 'https://c.example/cb?x=1 http:///nohost', 'https://c.example/cb?x=1 https://',
                    'https://c.example/cb?x=1 %zz', 'https://c.example/cb?x=1 a#f %zz /c', 'https://c.example/cb?x=1 a#f b#g /c', 'https://c.example/cb?x=1 /c a#f',
                    'https://c.example/cb?x=1 urn:ietf:wg:oauth:2.0:oob', 'https://c.example/cb?x=1 urn:ietf:wg:oauth:2.0:oob:auto', 'https://c.example/cb?x=1 //host/p',
                    'https://c.example/cb?x=1 mailto:a@b', 'https://c.example/cb?x=1 HTTP://UP.example/cb', 'https://c.example/cb?x=1 myapp://cb', "https://c.example/cb\nhttp://localhost/cb",
                    'https://c.example/cb?x=1  ', 'https://c.example/cb?x=1 javascript:alert(1)#f']
oauth_matches = [
  ['https://client.example/callback', 'https://client.example/callback'], ['https://client.example/callback', "http://localhost/cb\nhttps://client.example/callback"],
  ['HTTPS://client.example/callback', 'https://client.example/callback'], ['https://client.example:443/callback', 'https://client.example/callback'],
  ['http://127.0.0.1:9/cb?x=hello world', 'http://127.0.0.1/cb?x=hello%20world'], ['http://127.0.0.1:9/cb?x=hello%20world', 'http://127.0.0.1/cb?x=hello%20world'],
  ['http://127.0.0.1:9/cb?x=hello+world', 'http://127.0.0.1/cb?x=hello%20world'], ["http://127.0.0.1:9/cb?x=a\tb", 'http://127.0.0.1/cb?x=ab'],
  ["http://127.0.0.1:9/cb?x=a\r\nb", 'http://127.0.0.1/cb?x=ab'], ['http://127.0.0.1:9/cb?x=hello world', 'http://127.0.0.1:9/cb?x=hello world'],
  ['http://127.0.0.1:9/cb?x=hello', 'http://127.0.0.1/cb?x=hello world'], ['http://127.0.0.1:9/world', 'http://127.0.0.1/cb?x=hello http://127.0.0.1/world'],
  ['http://LOCALHOST:9/cb', 'http://localhost/cb'], ['HTTP://localhost:9/cb', 'http://localhost/cb'], ['http://localhost:9/cb?x=%7e', 'http://localhost/cb?x=~'],
  ['http://localhost:9/cb?x=%7E', 'http://localhost/cb?x=%7e'], ['http://localhost:09/cb', 'http://localhost:9/cb'], ['http://localhost:/cb', 'http://localhost:9/cb'],
  ['http://localhost:9/cb?x="', 'http://localhost/cb?x=%22'], ["http://localhost:9/cb?x='", 'http://localhost/cb?x=%27'], ['http://localhost:9/cb?', 'http://localhost/cb'],
  ['http://localhost:9/cb', 'http://localhost/cb?'], ['http://localhost:9', 'http://localhost/'], ['http://user@127.0.0.1:9/cb', 'http://127.0.0.1/cb'],
  ['http://[::1]:9/cb', 'http://[::1]/cb'], ['http://[::1]:9/cb', 'http://127.0.0.1/cb'], ['http://127.0.0.1:9/cb?x=%zz', 'http://127.0.0.1/cb?x=%zz'],
  ['//host/p', '//host/p'], ['myapp://callback', 'myapp://callback'], ['https://client.example/callback', ''], ['https://client.example/callback', "  \n"]
]
oauth_answers = [
  ['https://client.example/callback', { code: 'c0de', state: 'st' }], ['HTTPS://Client.Example:0443/Callback?x=1', { code: 'c0de', state: '' }],
  ['http://127.0.0.1:53211/cb?x=hello world', { code: 'c0de', state: 'a b&c="d"<e>+é' }], ['https://client.example/callback?keep=1&state=theirs', { code: 'c0de', state: nil }],
  ['https://client.example/callback?keep=1&state=theirs', { code: 'c0de', state: 'ours' }], ['https://client.example/cb?a=1&a=2&b&c=&d=%20&e=x', { code: 'c0de' }],
  ['https://client.example/cb?a&a=1', { code: 'c0de' }], ['https://client.example/cb?a=1&a', { code: 'c0de' }], ['https://client.example/cb?a=&a=1&&=v&x=y=z', { code: 'c0de' }],
  ['https://client.example/cb?code=theirs&a+b=c%2Fd&e=%FF', { code: 'c0de' }], ['https://client.example/cb?', { error: 'access_denied', error_description: 'The resource owner said no.', state: ' ' }],
  ['https://client.example/cb', {}], ['http://localhost:080/cb', { code: 'c0de' }], ['myapp://callback:9/x?y=1', { code: 'c0de' }], ['http://u:p@localhost/cb', { code: 'c0de' }]
]
oauth_built = lambda do |builder, url, parameters|
  Doorkeeper::OAuth::Authorization::URIBuilder.public_send(builder, url, parameters.dup)
rescue ArgumentError, Rack::QueryParser::InvalidParameterError
  nil
end
vectors['oauth_uri'] = {
  'parse' => oauth_uris.map { |text| { 'uri' => text, 'parts' => oauth_uri_parts.(text) } },
  'errors' => oauth_registered.map do |text|
    application = Doorkeeper::Application.new(name: 'x', redirect_uri: text, scopes: 'mcp', confidential: false)
    application.valid?
    raise "#{text}: #{application.errors.full_messages}" unless application.errors.attribute_names.all?(:redirect_uri)

    { 'redirect_uri' => text, 'errors' => application.errors.full_messages }
  end,
  'allowed' => oauth_matches.map do |url, registered|
    { 'url' => url, 'registered' => registered, 'allowed' => Doorkeeper::OAuth::Helpers::URIChecker.valid_for_authorization?(url, registered) }
  end,
  'answers' => oauth_answers.map do |url, parameters|
    { 'url' => url, 'parameters' => parameters.map { |name, value| [name.to_s, value.to_s] }, 'query' => oauth_built.(:uri_with_query, url, parameters),
      'fragment' => oauth_built.(:uri_with_fragment, url, parameters) }
  end
}
oauth_basic = Doorkeeper::OAuth::ClientAuthentication::ClientSecretBasic
vectors['oauth_basic'] = {
  'decode64' => ['YWJjOnM=', 'YW JjOnM=', 'YWJjOnM', "YW\nJj\tOnM=", 'YWJj=OnM=', '=YWJjOnM=', 'YWJjOnM===', 'Y', 'YQ', 'YQ=', 'YQ==', 'YWI', 'YWI=', 'YWJj', 'YW-Jj_OnM', 'YWJjOnM=YWJj',
                 '!!!!', '', 'Basic', 'YQ==YQ==', 'YWJ=jOnM', 'Y=WJj', '/+/+', '////', 'AAAA', 'é', 'YWJjZGVmZ2hpamtsbW5vcA==', 'Y Q = ='].map { |text| { 'text' => text, 'bytes' => Base64.decode64(text).bytes } },
  'credentials' => ['Basic YWJjOnM=', 'Basic YW JjOnM=', 'Basic  YWJjOnM=', "basic\tYWJjOnM=", 'BASIC YWJjOnM=', 'bAsIc YWJjOnM=', 'Basic ', 'Basic', 'Basic  ', 'Basic OnM=', 'Basic IDpz',
                    'Basic IAk6cw==', 'Basic YWJj', 'Basic YWJjOg==', 'Basic YWJjOnM6dA==', "Basic\vYQ==", "Basic \tYQ==", 'Basic YQ==YQ==', 'Basic !!!!', 'Bearer YWJjOnM=', ' Basic YWJjOnM=', '',
                    'Basic YWJjOnM= trailing', 'Basic YTpiIGM='].map do |header|
    { 'authorization' => header, 'credentials' => oauth_basic.send(:credentials_from, Struct.new(:authorization).new(header)) }
  end
}
# What the bot list and the bot page print (rust/src/web/format.rs, colors.rs, ring.rs,
# engine/schedule.rs). Floats travel as JSON numbers, BigDecimals as strings.
helpers = ApplicationController.helpers
strip = ->(decimal) { decimal.to_s('F').sub(/([0-9]\d*)\.0$/, '\1') }
bot_floats = [0.0, 1.0, 60.0, 0.6 * 100, 0.07 * 100, 100.0, 12.345678901234567, 1e-5, 0.0001, 0.00012345, 1e15, 1e16, 123_456_789_012_345_680.0,
              1.5e-7, -2.5, 0.1 + 0.2, 33.333333333333336, 2_629_746.0 / 3, 1e22, 0.3, -0.0, 56.548667764616276, 1234.5,
              999_999_999_999_999.0, 1_234_567_890_123_456.0, 100_000_000_000_000.0, 123_456_789_012_345.6, 0.001, 0.00099]
stored_numbers = [50, 25.5, 0.01, 0.005, 1000, 5.0, 1e-7, 123_456.789, 0.1 + 0.2, 1_000_000, 0, 2.0]
zone_names = ActiveSupport::TimeZone::MAPPING.keys
instants = %w[2026-09-10T12:00:30Z 2026-01-15T03:04:05Z 2026-03-29T00:59:59Z 2026-03-29T01:00:00Z 2026-11-01T05:59:00Z]
ring_sets = [[['5000', '#F7931A'], ['120', nil]], [['100', '#1A2B3C'], ['1', '#FFFFFF'], ['1', '#000000'], ['0.5', nil]],
             [['50', '#76B900'], ['50.5', '#0668E1'], ['0.9', '#E31837']], [['10', nil]], [['0', '#111111']],
             (1..40).map { |n| [(41 - n).to_s, format('#%06x', n * 400_000)] }, [['97', '#ED1C24'], ['3', '#050505']],
             [['1234.56789', '#abcdef'], ['987.654321', '#123456'], ['12.5', '#FEDCBA'], ['12.25', '#777777'], ['7', '#00ff00']]]
progress_cases = [['2026-09-10T09:00:00Z', '2026-09-11T09:00:00Z'], ['2026-09-09T12:00:30.123456Z', '2026-09-10T12:00:31Z'],
                  ['2026-09-10T11:55:30.5Z', '2026-09-10T12:25:30Z'], ['2026-09-03T12:00:30.123456Z', '2026-09-17T12:00:30.123456Z'],
                  ['2026-09-10T12:00:30.123456Z', '2026-09-10T13:00:00Z'], ['2026-09-10T12:00:31Z', '2026-09-10T13:00:00Z'],
                  ['2026-09-10T11:00:00Z', '2026-09-10T11:00:00Z'], ['2026-08-10T12:00:30.123Z', '2026-09-10T12:00:30.124Z']]
progress_now = Time.iso8601('2026-09-10T12:00:30.123456Z')
dotiw_seconds = [0, 29, 30, 89, 90, 300, 2640, 5399, 5400, 8640.0, 86_399, 86_400, 90_000, 151_199, 604_800, 1_296_000, 2_419_199, 2_419_200,
                 2_591_999, 2_592_000, 2_629_746, 2_629_745.9, 262_974.6, 60_480.0, 360.0, 43_200, 172_800.0, 3661, 7322, 2_500_000.5]
dotiw_nows = %w[2026-09-10T12:00:30Z 2026-01-31T23:30:00Z 2026-02-01T00:00:00Z 2028-02-10T05:00:00Z 2026-12-31T12:00:00Z]
# Past a year: a monthly 100 in slices of 1,200 is one order a year, and Rails sets no upper bound on the slice.
# The last is a thousand years, the longest span a page prints (web::bot::MAX_SPAN_SECONDS).
dotiw_years = [31_556_952, 31_622_400.0, 40_000_000, 63_113_904, 100_000_000.5, 157_784_760, 668_000_000, 31_556_952_000]
# What a path segment or a cursor's id may be: String#to_i reads both.
integer_texts = ['1', '12abc', 'abc', ' 7', '1_0', '-3', '0', '007', '1.turbo_stream', '99999999999999999999', '', '+5', '3 4', '1__0', '_1',
                 '9223372036854775807', '9223372036854775808', '12345678901234567', "\u00A01", "\t7", "\n8", "\v9", "\f3", "\r4", '+10', ' +5',
                 '+ 5', '--5', '+-5', '-0', "\uFF11\uFF12", "1\u0663", "\u20281", '1_', '1_a', '-1_000', '-99999999999999999999', '0x1A', '1e3',
                 '-9223372036854775808', '-9223372036854775809'].freeze
# Checkpoints at microseconds of every kind: most are not a double's, and about half of those come back a microsecond early.
job_checkpoints = (%w[2026-09-11T12:00:30 2026-12-31T23:59:59 2027-03-28T01:00:00 2031-07-04T08:15:42 2038-01-19T03:14:07].product(
  %w[000000 000001 000456 000457 123456 250000 333333 499999 500000 666667 999998 999999]
).map { |second, fraction| "#{second}.#{fraction}Z" } + %w[2026-09-11T12:00:30Z 1999-12-31T23:59:59.000456Z]).freeze
# A Float as its exact bits: the JSON writer keeps only 16 digits of one.
fl = ->(number) { number.is_a?(Float) ? { 'bits' => [number].pack('G').unpack1('H*') } : number }
# Checkpoints off the microsecond grid: an anchor out of a row plus a Float, as Automation::Schedulable adds one
# (`checkpoint + intervals * duration.to_f`, or `checkpoint += duration` over and over; `checkpoint - duration.to_f`
# for the last one). The first is the reviewed case: 7 a day in slices of 1. Durations under 2,048 seconds, whole
# nanoseconds, and anchors past 2038 and 2106 each take another branch of Ruby's Rational#to_f.
exact_rng = Random.new(3_141_592)
exact_anchors = %w[2026-10-01T12:00:30.000456Z 2026-09-10T11:00:30.000456Z 2026-09-10T12:00:30Z 2026-03-29T00:59:59.999999Z
                   2038-01-19T03:14:07.500001Z 2110-06-01T00:00:00.123457Z].freeze
exact_durations = [86_400.0 / (7 / 1.0), 86_400.0 / (7 / 1.0)] +
                  [[3_600.0, 7, 1.0], [3_600.0, 11, 1.5], [86_400.0, 25.5, 2.0], [86_400.0, 100, 0.37], [604_800.0, 60, 0.03], [604_800.0, 1000, 950.0],
                   [2_629_746.0, 1000, 333.0], [2_629_746.0, 200, 200.0], [86_400.0, 2, 1.0], [3_600.0, 3, 0.25]].map { |interval, amount, slice| interval / (amount / slice) } +
                  [43_200.0, 100.001953125, 300.5, 0.75, 1.0000000000000002] + Array.new(12) { exact_rng.rand * [3_000.0, 90_000.0, 700_000.0].sample(random: exact_rng) + 1 }
exact_cases = [[exact_anchors.first, exact_durations.first, 1, '2026-10-01T13:00:30Z']] +
              exact_anchors.product(exact_durations.drop(1)).flat_map do |anchor, duration|
                [1, -1, exact_rng.rand(2..40)].map do |times|
                  # As Rails builds a step: one Float product for a duration in seconds, added once.
                  float, count = times > 1 && duration != 2_629_746.0 ? [times * duration, 1] : [duration, times]
                  [anchor, float, count, (Time.iso8601(anchor) + exact_rng.rand(-90_000.0..90_000.0)).round(6).utc.iso8601(6)]
                end
              end
vectors['bot_pages'] = {
  'float_to_s' => bot_floats.map { |float| { 'float' => fl.(float), 'text' => float.to_s } },
  'float_round' => [[0.6 * 100, 1], [0.07 * 100, 1], [2.675, 2], [1.005, 2], [25.5, 2], [0.125, 2], [1234.5678, 2], [5.0, 2], [0.045, 2], [1e-9, 9],
                    [123.456, 9], [-2.675, 2], [0.285, 2], [1.15, 1], [8.345, 2], [56.548667764616276, 2], [0.5, 1], [14.137166941154069, 2],
                    [1e20, 2], [4.35, 1], [1000.4999, 2], [0.3 - 0.1, 2],
                    # Up to 14 digits, the most a served ticker states (web::bot::MAX_DECIMALS) and the last MRI rounds by a power of ten.
                    [0.12345678901234567, 14], [0.1 + 0.2, 14], [1234.5678901234567, 12], [2.5e-14, 14], [100.0 / 3, 14], [1e-15, 14],
                    [0.000123456789012345, 14], [5.0e-15, 14], [0.123456789012345, 14], [987_654.32109876543, 10], [7.5e-12, 11],
                    [-0.98765432109876543, 13], [49.999999999999995, 14]].map do |float, digits|
    { 'float' => fl.(float), 'digits' => digits, 'rounded' => fl.(float.round(digits)) }
  end,
  'float_round_whole' => [0.5, 1.5, 2.5, -0.5, 24.999, 25.0, 49.5, 99.99999].map { |float| { 'float' => fl.(float), 'rounded' => float.round } },
  'input_value' => stored_numbers.map { |number| { 'number' => fl.(number), 'text' => strip.(number.to_d) } },
  'times_100' => [0.01, 0.005, 0.001, 0.2, 0.07, 0.15, 1, 0.0015].map { |number| { 'number' => fl.(number), 'text' => strip.(number.to_d * 100) } },
  'to_s' => (stored_numbers + [BigDecimal('50'), BigDecimal('0.121250333'), BigDecimal('1E-9'), BigDecimal('123456789.5')]).map do |number|
    { 'number' => number.is_a?(BigDecimal) ? number.to_s('F') : fl.(number), 'decimal' => number.is_a?(BigDecimal), 'text' => number.to_s,
      'rounded' => number.round(2).to_s }
  end,
  'number_with_precision' => [[100.0, 1, false], [60.00000000000001, 1, false], [1_234_567.891, 2, true], [0.005, 2, true], [49.999999, 2, false],
                              [BigDecimal('49.999999'), 2, false], [BigDecimal('412.37'), 2, false], [BigDecimal('1234.005'), 2, true],
                              [BigDecimal('0.4'), 2, false], [-1234.5, 1, true], [99.95, 1, false], [50, 2, false], [0.6 + 0.4, 1, false],
                              [BigDecimal('50'), 2, false], [1e-7, 2, false]].map do |number, precision, delimited|
    { 'number' => number.is_a?(BigDecimal) ? number.to_s('F') : fl.(number), 'decimal' => number.is_a?(BigDecimal), 'precision' => precision,
      'delimited' => delimited,
      'text' => delimited ? helpers.number_with_precision(number, precision:, delimiter: ',') : helpers.number_with_precision(number, precision:) }
  end,
  # quote_amount_limit - what was spent, then `[it, 0].max` and `.round(2)`, as the amount-limit info prints it.
  'limit_left' => [[1000, 0], [1000, BigDecimal('49.999999')], [1000.5, 0], [1000.5, BigDecimal('49.999999')], [100, BigDecimal('150')],
                   [0.1, BigDecimal('0.03')], [1000, BigDecimal('0')], [250.75, BigDecimal('250.75')]].map do |limit, spent|
    left = [limit - spent, 0].max
    { 'limit' => fl.(limit), 'spent' => spent.is_a?(BigDecimal) ? spent.to_s('F') : nil, 'text' => left.round(2).to_s, 'reached' => left < 0.01 }
  end,
  # The dotiw gem replaces Rails' distance_of_time_in_words. Up to four weeks the words depend on the
  # seconds alone; from there on dotiw counts calendar months from the present moment.
  'distance_of_time' => (dotiw_nows.product(dotiw_seconds, %w[en de pl ru cs]) +
                         dotiw_nows.first(2).product(dotiw_years, I18n.available_locales.map(&:to_s))).map do |now, seconds, locale|
    text = travel_to(Time.iso8601(now)) { I18n.with_locale(locale) { helpers.distance_of_time_in_words(seconds.seconds) } }
    { 'now' => now, 'seconds' => fl.(seconds), 'locale' => locale, 'text' => text }
  end,
  # Its unit names, from the gem's own locale files: the crate pins them (web::i18n FROM_GEMS).
  'dotiw' => I18n.available_locales.flat_map do |locale|
    units = I18n.backend.send(:translations).dig(locale, :datetime, :dotiw) || {}
    units.slice(:seconds, :minutes, :hours, :days, :weeks, :months, :years).flat_map do |unit, forms|
      # A locale may give a unit one text for every count (the gem's Danish year).
      forms.is_a?(Hash) ? forms.map { |form, text| ["#{locale}.datetime.dotiw.#{unit}.#{form}", text] } : [["#{locale}.datetime.dotiw.#{unit}", forms]]
    end + (units[:less_than_x] ? [["#{locale}.datetime.dotiw.less_than_x", units[:less_than_x]]] : [])
  end.to_h,
  'zones' => instants.to_h { |at| [at, zone_names.to_h { |name| [name, Time.iso8601(at).in_time_zone(name).strftime('%Z')] }] },
  'table_when' => instants.product(%w[UTC Tallinn Warsaw Hawaii Kathmandu], %w[en de]).map do |at, zone, locale|
    time = Time.iso8601(at)
    { 'at' => at, 'zone' => zone, 'locale' => locale, 'date' => helpers.table_date(time, zone),
      'clock' => I18n.with_locale(locale) { helpers.table_clock(time, zone) }, 'iso8601' => time.utc.iso8601,
      'datetime_local' => time.in_time_zone(zone).strftime('%Y-%m-%dT%H:%M') }
  end,
  'iso8601' => %w[2026-09-10T12:00:30.999999Z 2026-09-10T12:00:30Z].map { |at| { 'at' => at, 'text' => Time.iso8601(at).utc.iso8601 } },
  'start_default' => (instants + %w[2026-09-14T13:29:59Z 2026-09-14T13:30:00Z 2026-09-13T03:59:59Z 2026-09-13T04:00:00Z]).product(
    ['UTC', 'Tallinn', 'Hawaii', 'Tokyo', 'Eastern Time (US & Canada)', 'Sydney']
  ).map do |at, zone|
    mode, time = Bots::DcaMultiAsset.new(user: User.new(time_zone: zone)).default_start_time_selection(now: Time.iso8601(at))
    { 'at' => at, 'zone' => zone, 'mode' => mode, 'time' => time }
  end,
  'ensure_contrast' => %w[#1A2B3C #76B900 #F5F5F7 #0668E1 #050505 #E31837 #ED1C24 #8A9BA8 #FFFFFF #000000 #7f7f7f #abcdef #ABCDEF 808080 #0a0a0a
                          #101010 #c8c8c8 #d9d9d9 #zzzzzz #12345 #1234567].map do |color|
    { 'color' => color, 'contrast' => helpers.ensure_contrast(color) }
  end + [{ 'color' => '', 'contrast' => helpers.ensure_contrast('') }],
  'ticker_class' => [['Stock', nil], ['Stock', '#111111'], ['Stock', ''], ['Cryptocurrency', nil], [nil, nil], ['ETF', nil]].map do |category, color|
    { 'category' => category, 'color' => color, 'class' => helpers.ticker_class_for(category:, color:) }
  end,
  'ring' => ring_sets.map do |pairs|
    { 'values' => pairs, 'arcs' => helpers.send(:icon_arcs, pairs.map { |value, color| [BigDecimal(value), color] }).map { |arc| arc.slice(:color, :dash, :offset).transform_values(&:to_s) } }
  end,
  # Automation::Schedulable#progress_percentage, and the width the status bar prints from it.
  'progress' => progress_cases.map do |from, to|
    start_time, end_time = Time.iso8601(from), Time.iso8601(to)
    share = start_time.present? && end_time > start_time ? (progress_now - start_time) / (end_time - start_time) : 0
    { 'now' => progress_now.iso8601(6), 'from' => from, 'to' => to, 'width' => "#{share * 100}" }
  end,
  # The time a job enqueued for a checkpoint is held at: ActiveJob gives the adapter `wait_until.to_f`, Solid Queue reads
  # it with Time.at, and the column cuts it to six decimals (web::bot::status::job_time_us).
  'job_time' => job_checkpoints.map do |text|
    held = SolidQueue::Job.type_for_attribute(:scheduled_at).serialize(Time.at(Time.iso8601(text).to_f))
    { 'checkpoint' => text, 'job' => held.utc.iso8601(6) }
  end,
  # A checkpoint off the microsecond grid (`Time + Float` is exact): the time its job is held at, the second `iso8601`
  # prints, Time#to_f, and Time#- from another time and to it (web::bot::status::Instant). MRI's Rational#to_f is not
  # the nearest Float; these hold the port to its own steps.
  'exact_times' => exact_cases.map do |anchor, float, times, other|
    at = times.abs.times.reduce(Time.iso8601(anchor)) { |time, _| times.negative? ? time - float : time + float }
    held = SolidQueue::Job.type_for_attribute(:scheduled_at).serialize(Time.at(at.to_f))
    now = Time.iso8601(other)
    { 'anchor' => anchor, 'float' => fl.(float), 'times' => times, 'other' => other, 'job' => held.utc.iso8601(6), 'second' => at.utc.iso8601,
      'to_f' => fl.(at.to_f), 'since' => fl.(now - at), 'until' => fl.(at - now) }
  end,
  'string_to_id' => integer_texts.map do |text|
    id = begin
      Bot.type_for_attribute(:id).serialize(text) # what `find` binds; out of the column's range it raises, and `find` finds nothing
    rescue ActiveModel::RangeError
      nil
    end
    { 'text' => text, 'id' => id }
  end,
  # String#to_i itself, as a string: Ruby has no largest Integer.
  'string_to_i' => integer_texts.map { |text| { 'text' => text, 'integer' => text.to_i.to_s } }
}
vectors['action_transport'] = action_transport_vectors
# users.time_zone holds one of these names; the crate embeds the table (src/web/time_zones.json).
time_zones = ActiveSupport::TimeZone::MAPPING

# Exchanges::Alpaca sizing and wire for a stock or ETF (category Stock): the importers' MarketData::STOCK_TICKER_DEFAULTS
# (market_data.rb:381-389) and time_in_force 'day' (exchanges/alpaca.rb:910, :953). rust/tests/amount.rs.
alpaca = Exchanges::Alpaca.new
captured = nil
stock_client = Object.new
stock_client.define_singleton_method(:create_order) { |**kw| captured = kw; Result::Success.new('id' => 'X') }
alpaca.define_singleton_method(:client) { stock_client }
stock = Asset.new(symbol: 'AAPL', category: 'Stock')
alpaca_stock_sizing = []
%w[187.43 0.5123 1234.5].each do |price_s|
  %w[60 0.99 1 5.005 123.456789 1000000].each do |x_s|
    %i[market_order limit_order].each do |order_type|
      ticker = Ticker.new(exchange: alpaca, ticker: 'AAPL', base: 'AAPL', quote: 'USD', base_asset: stock, base_decimals: 9, quote_decimals: 2,
                          price_decimals: 2, minimum_base_size: BigDecimal('0.000000001'), minimum_quote_size: BigDecimal('1'))
      bot = Bots::DcaMultiAsset.new(exchange: alpaca)
      price = order_type == :limit_order ? ticker.adjusted_price(price: BigDecimal(price_s) * (1.to_d - 0.0025.to_d)) : BigDecimal(price_s)
      x = BigDecimal(x_s)
      info = bot.send(:calculate_best_amount_info, { ticker:, price:, amount: x / price, quote_amount: x, side: :buy, order_type: })
      captured = nil
      if order_type == :limit_order
        alpaca.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price:)
      else
        alpaca.market_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type])
      end
      alpaca_stock_sizing << { 'last_or_ask' => price_s, 'x' => x_s, 'order_type' => order_type.to_s, 'price' => price.to_s('F'),
                               'below_minimum' => info[:below_minimum_amount], 'wire' => captured.transform_keys(&:to_s).transform_values(&:to_s) }
    end
  end
end
vectors['alpaca_stock_sizing'] = alpaca_stock_sizing

# ActiveSupport's Time#as_json for Exchanges::Alpaca#next_market_open_at (`Time.parse(clock['next_open'])`), as the
# market_closed activity's details store it (rust/src/ruby.rs time_as_json).
vectors['time_as_json'] = ['2026-09-08T09:30:00-04:00', '2026-11-09T09:30:00-05:00', '2026-09-08T13:30:00Z', '2026-09-08T13:30:00+00:00',
                           '2026-09-08T09:30:00.123456-04:00', '2026-09-08T09:30:00.9999-04:00']
                          .to_h { |s| [s, JSON.parse({ 't' => Time.parse(s) }.to_json)['t']] }

# Bot::Composition::Measurable#metrics' asset_breakdown amounts and restated_at, and Bot::Restatable#restated_prices_untrusted?,
# over recorded split rows (rust/tests/splits.rs). Each case: `assets` (an Alpaca stock per `base`, with its `symbol`),
# `allocations` (bases), `buys` ([base, created_at, amount_exec, price], closed REGULAR buys recording the asset's symbol), and
# `splits` (account_transactions adjustments marked 'split': base_currency `name`, transacted_at `at`, split_ratio `ratio`,
# for `user` self or other, on `venue` alpaca or kraken). Recorded at SPLIT_NOW inside a transaction that is rolled back.
self.extend ActiveSupport::Testing::TimeHelpers
SPLIT_NOW = Time.utc(2026, 9, 10, 12)
w = ->(base, symbol = base) { { 'base' => base, 'symbol' => symbol } }
split = ->(name, at, ratio, user: 'self', venue: 'alpaca') { { 'name' => name, 'at' => at, 'ratio' => ratio, 'user' => user, 'venue' => venue } }
buy = ->(base, at, amount) { [base, at, amount, '100'] }
one_buy = [buy.('WAAA', '2026-09-01 14:00:01', '10')]
split_cases = [
  { 'name' => 'no_split', 'splits' => [] },
  { 'name' => 'forward_merged', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'reverse_merged', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '1:10')] },
  { 'name' => 'lone_leg_then_merged', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', nil), split.('WAAA', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'disagreeing_sources', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1'), split.('WAAA', '2026-09-05 04:00:00', '5:1')] },
  { 'name' => 'lone_leg_no_ratio', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', nil)] },
  { 'name' => 'malformed_ratio', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:0')] },
  { 'name' => 'future_dated', 'splits' => [split.('WAAA', '2026-09-15 00:00:00', '10:1')] },
  { 'name' => 'before_first_buy', 'splits' => [split.('WAAA', '2026-08-30 00:00:00', '10:1')] },
  { 'name' => 'between_buys', 'buys' => one_buy + [buy.('WAAA', '2026-09-08 14:00:01', '5')], 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'same_instant_as_an_order', 'buys' => one_buy + [buy.('WAAA', '2026-09-05 00:00:00', '5')], 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'recent', 'splits' => [split.('WAAA', '2026-09-09 12:00:00', '10:1')] },
  { 'name' => 'exactly_two_days', 'splits' => [split.('WAAA', '2026-09-08 12:00:00', '10:1')] },
  { 'name' => 'basket_one_member_splits', 'assets' => [w.('WAAA'), w.('WBBB')], 'allocations' => %w[WAAA WBBB],
    'buys' => one_buy + [buy.('WBBB', '2026-09-01 14:00:02', '7')], 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'spelled_differently', 'assets' => [w.('WCC.X', 'WCCC')], 'allocations' => %w[WCC.X], 'buys' => [buy.('WCC.X', '2026-09-01 14:00:01', '10')],
    'splits' => [split.('WCC.X', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'a_name_two_assets_share', 'assets' => [w.('WDUP'), w.('WDUQ', 'WDUP')], 'allocations' => %w[WDUP], 'buys' => [buy.('WDUP', '2026-09-01 14:00:01', '10')],
    'splits' => [split.('WDUP', '2026-09-05 00:00:00', '10:1')] },
  { 'name' => 'fractional', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '3:2')] },
  { 'name' => 'another_users_split', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1', user: 'other')] },
  { 'name' => 'a_venue_never_traded', 'splits' => [split.('WAAA', '2026-09-05 00:00:00', '10:1', venue: 'kraken')] }
].map { |k| { 'assets' => [w.('WAAA')], 'allocations' => %w[WAAA], 'buys' => one_buy }.merge(k) }
vectors['split_walks'] = split_cases.map do |kase|
  expected = nil
  ActiveRecord::Base.transaction do
    owner, stranger = %w[split-owner split-stranger].map do |n|
      User.new(name: n, email: "#{n}@example.com", password: 'correct horse battery staple', confirmed_at: Time.current).tap { |u| u.save!(validate: false) }
    end
    alpaca = Exchanges::Alpaca.first || Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    kraken = Exchanges::Kraken.first || Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    usd = Asset.create!(external_id: 'usd-w', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    assets = kase['assets'].to_h do |a|
      asset = Asset.create!(external_id: "#{a['base']}.W", symbol: a['symbol'], name: a['symbol'], category: 'Stock', instrument_type: 'stock')
      [asset, usd].each { |listed| ExchangeAsset.find_or_create_by!(exchange: alpaca, asset: listed) { |ea| ea.available = true } }
      Ticker.create!(exchange: alpaca, ticker: a['base'], base: a['base'], quote: 'USD', base_asset: asset, quote_asset: usd, base_decimals: 9,
                     quote_decimals: 2, price_decimals: 2, minimum_base_size: BigDecimal('0.000000001'), minimum_quote_size: 1)
      [a['base'], asset]
    end
    weight = 1.0 / kase['allocations'].size
    bot = Bots::DcaMultiAsset.new(user: owner, exchange: alpaca, status: :stopped, settings: {
      'quote_asset_id' => usd.id, 'quote_amount' => 60.0, 'interval' => 'week', 'weighting' => 'manual',
      'allocations' => kase['allocations'].to_h { |b| [assets.fetch(b).id.to_s, weight] } })
    bot.set_missed_quote_amount
    bot.save!(validate: false)
    kase['buys'].each do |base, at, amount, price|
      asset = assets.fetch(base)
      Transaction.insert!({ 'bot_id' => bot.id, 'exchange_id' => alpaca.id, 'external_id' => "W-#{base}-#{at}", 'status' => 0, 'external_status' => 2,
                            'side' => 0, 'order_type' => 0, 'amount' => amount, 'price' => price, 'amount_exec' => amount,
                            'quote_amount' => (BigDecimal(amount) * BigDecimal(price)).to_s('F'), 'quote_amount_exec' => (BigDecimal(amount) * BigDecimal(price)).to_s('F'),
                            'base' => asset.symbol, 'quote' => 'USD', 'base_asset_id' => asset.id, 'quote_asset_id' => usd.id, 'transaction_type' => 'REGULAR',
                            'bot_interval' => 'week', 'bot_quote_amount' => 60, 'error_messages' => [], 'created_at' => at, 'updated_at' => at })
    end
    kase['splits'].each do |s|
      raw = { 'activity_type' => 'SPLIT', 'symbol' => s['name'], 'corporate_action' => 'split' }
      raw['split_ratio'] = s['ratio'] if s['ratio']
      AccountTransaction.insert!({ 'user_id' => (s['user'] == 'other' ? stranger : owner).id, 'exchange_id' => (s['venue'] == 'kraken' ? kraken : alpaca).id,
                                   'entry_type' => 15, 'base_currency' => s['name'], 'base_amount' => '0', 'transacted_at' => s['at'], 'raw_data' => raw,
                                   'created_at' => s['at'], 'updated_at' => s['at'] })
    end
    travel_to(SPLIT_NOW) do
      data = bot.metrics(force: true)
      base_of = assets.to_h { |base, asset| [asset.id, base] }
      expected = { 'amounts' => data[:asset_breakdown].to_h { |key, v| [base_of.fetch(data[:key_assets].fetch(key)), v[:amount].to_d.to_s('F')] },
                   'restated_at' => data[:restated_at]&.utc&.iso8601(6), 'untrusted' => bot.restated_prices_untrusted?(data) }
    end
    raise ActiveRecord::Rollback
  end
  kase.merge('now' => SPLIT_NOW.iso8601, 'expected' => expected)
end

# Bot::Composition::Weightable.blend: market-cap weights blended toward equal by allocation_flattening, in Float, in the
# caps' order (rust/src/engine/index.rs blend). Integer keys stand in for asset ids.
vectors['index_blends'] = [
  [[5e12, 3e12, 2e12], 0.0], [[5e12, 3e12, 2e12], 0.5], [[5e12, 3e12, 2e12], 1.0], [[1.0, 1.0, 1.0], 0.0], [[0.1, 0.2, 0.3], 0.0],
  [[3.4e12, 3.1e12, 2.9e12, 2.1e12, 1.9e12, 1.6e12, 1.2e12, 1.0e12, 0.9e12, 0.8e12], 0.0],
  [[3.4e12, 3.1e12, 2.9e12, 2.1e12, 1.9e12, 1.6e12, 1.2e12, 1.0e12, 0.9e12, 0.8e12], 0.3],
  [[0.0, 0.0], 0.0], [[0.0, 5.0], 0.25], [[7.0], 0.0], [[1e12, 3e-3], 0.9], [[123_456_789.123, 987_654_321.987, 55_555.5], 0.123]
].map do |caps, f|
  { 'caps' => caps, 'flattening' => f,
    'weights' => Bot::Composition::Weightable.blend(market_caps: caps.each_with_index.to_h { |cap, i| [i, cap] }, flattening: f).values }
end

vectors['index_duplicate_blend'] = Bots::DcaIndex.new(allocation_flattening: 0.5).calculate_allocations_with_flattening(
  [{ asset_id: 1, ticker_id: 1, market_cap: 100 }, { asset_id: 1, ticker_id: 1, market_cap: 100 }, { asset_id: 2, ticker_id: 2, market_cap: 10 }]
).map { |row| row.merge(weight: BotIndexAsset.type_for_attribute('target_allocation').cast(row[:weight]).round(6).to_f) }

File.write(Rails.root.join('rust/src/web/time_zones.json'), "#{JSON.pretty_generate(time_zones)}\n")
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vectors)}\n")
puts "wrote #{ARGV.fetch(0)}"
