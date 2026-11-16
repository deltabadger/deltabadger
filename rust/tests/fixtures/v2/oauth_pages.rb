# The part of script/rust/pages.rb (and schema_template.rb) at tag rails-oracle-final that fixtures/v2/oauth.rb uses,
# for rust/tests/oauth.rs on 2.70.21 in the published image (common::v2): pages.rb whole cannot load there (its
# settings half needs webmock, which the production bundle lacks). Copied as the tag has it, except that `build` sets
# up only a scenario's user (all an OAuth scenario has) and refuses anything else, the primary schema is read from
# /x/baseline.sql (rust/schema/baseline.sql, copied there by the test), and verify_rendered! reads asset paths from the
# image's precompiled manifest.
require 'json'
require 'digest'
require 'fileutils'
require 'active_support/testing/time_helpers'

module SchemaTemplate
  PRIMARY, QUEUE = 'production.sqlite3', 'production_queue.sqlite3'

  def self.load_primary(path)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
    db = ActiveRecord::Base.connection.raw_connection
    db.execute('PRAGMA journal_mode=WAL')
    db.transaction(:immediate) { db.execute_batch(File.read('/x/baseline.sql')) }
  end

  def self.load_queue(path)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: path)
    ActiveRecord::Schema.verbose = false
    load Rails.root.join('db/queue_schema.rb')
  end
end

module Pages
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  PASSWORD = 'Correct-horse-9'.freeze
  OTP_SEED = 'JBSWY3DPEHPK3PXP'.freeze
  # Mid-minute, so a rate-limit window does not roll over during a scenario; with microseconds, so a
  # timestamp written without them shows.
  AT = '2026-09-10T12:00:30.123456Z'.freeze

  def user(attrs = {})
    { 'name' => 'Owner', 'email' => 'owner@example.com', 'admin' => true, 'setup_completed' => true,
      'confirmed_at' => '2026-01-01T00:00:00Z' }.merge(attrs)
  end

  def two_factor_user(attrs = {}) = user({ 'otp_module' => 1, 'otp_secret_key' => OTP_SEED }.merge(attrs))
  def code_at(time) = ROTP::TOTP.new(OTP_SEED).at(Time.iso8601(time))

  def connect(dir) = ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))

  # An install as Rails prepares one, built once and copied per scenario.
  def template(root)
    dir = File.join(root, '.template')
    FileUtils.mkdir_p(dir)
    SchemaTemplate.load_primary(File.join(dir, SchemaTemplate::PRIMARY))
    SchemaTemplate.load_queue(File.join(dir, SchemaTemplate::QUEUE))
    ActiveRecord::Base.connection_pool.disconnect!
    dir
  end

  def build(dir, template, scenario)
    unknown = scenario.keys - %w[user steps sql allowed_hosts]
    raise "oauth_pages.rb builds users only, not #{unknown}" if unknown.any?

    FileUtils.mkdir_p(dir)
    %w[production.sqlite3 production_queue.sqlite3].each { |file| FileUtils.cp(File.join(template, file), File.join(dir, file)) }
    connect(dir)
    travel_to(Time.iso8601('2026-01-01T00:00:00Z')) do # created_at and updated_at of the seeded rows
      [scenario['user'] || user].each do |attrs|
        times = %w[confirmed_at locked_at last_otp_at remember_created_at].to_h { |column| [column, attrs[column] && Time.iso8601(attrs[column])] }
        User.new(attrs.merge(times).merge('password' => PASSWORD)).save!(validate: false)
      end
    end
    ActiveRecord::Base.connection_pool.disconnect!
    {}
  end

  def meta_token(page) = page.to_s[/<meta name="csrf-token" content="([^"]+)"/, 1]

  # [opening tag, inner markup] of every form on the page.
  def forms(page) = page.to_s.scan(%r{<form\b([^>]*)>(.*?)</form>}m)

  # The hidden authenticity_token of the first form on `page` that posts to `action`.
  def form_token(page, action)
    forms(page).filter_map { |tag, inner| inner[/name="authenticity_token" value="([^"]+)"/, 1] if tag.include?(%( action="#{action}")) }.first
  end

  # The comparison masks CSRF tokens and stream signatures, and a mask must not hide a broken value. So
  # every token and every signed stream name Rails rendered is checked here, with Rails' own code: the
  # meta tag against the session's token, each form's field against that or the form's own token
  # (Rails issues one per action and method), each stream name against Turbo's verifier.
  def verify_rendered!(where, session)
    body = session.response.body
    controller = session.controller
    genuine = lambda do |token, action, method|
      unmasked = controller.send(:unmask_token, controller.send(:decode_csrf_token, token))
      accepted = [controller.send(:global_csrf_token, session.request.session)]
      accepted << controller.send(:per_form_csrf_token, session.request.session, action.split('?').first.chomp('/'), method) if action
      accepted.any? { |real| ActiveSupport::SecurityUtils.fixed_length_secure_compare(unmasked, real) }
    end
    raise "#{where}: the csrf-token meta tag does not verify" if meta_token(body) && !genuine.(meta_token(body), nil, nil)

    forms(body).each do |tag, inner|
      token = inner[/name="authenticity_token" value="([^"]+)"/, 1] or next
      action = tag[/ action="([^"]*)"/, 1]
      method = (inner[/name="_method" value="([^"]*)"/, 1] || tag[/ method="([^"]*)"/, 1]).upcase
      raise "#{where}: the token of the form for #{method} #{action} does not verify" unless genuine.(token, action, method)
    end
    body.scan(/signed-stream-name="([^"]+)"/).flatten.each do |signed|
      raise "#{where}: a signed stream name does not verify" unless Turbo::StreamsChannel.verified_stream_name(signed)
    end
    # Asset fingerprints are masked too, so each reference is looked up: it must be exactly the path
    # the image's precompiled manifest gives that asset (the tag compiled it with Sprockets, which needs sassc here).
    body.scan(%r{/assets/([^"'?\s]+)}).flatten.uniq.each do |path|
      logical = path.sub(/-\h{64}(\.[^.\/]+)\z/, '\1')
      built = Rails.application.assets_manifest.assets[logical]
      raise "#{where}: the page refers to /assets/#{path}, and Sprockets has #{built.inspect}" unless built == path
    end
  end
end
