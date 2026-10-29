require 'test_helper'

class ExportPortableTest < ActiveSupport::TestCase
  test 'every encrypted model has an explicit portable policy' do
    assert_equal EncryptedCredentials::COVERAGE.keys.sort, Export::Portable::ENCRYPTED_POLICIES.keys.sort
    assert_equal %w[Rules::Withdrawal User], Export::Portable::ENCRYPTED_POLICIES.select { |_model, policy| policy == :keep }.keys.sort
    assert_equal [:otp_secret_key], EncryptedCredentials::COVERAGE.fetch('User').fetch(:attributes)
    assert_equal [:address], EncryptedCredentials::COVERAGE.fetch('Rules::Withdrawal').fetch(:attributes)
  end

  test 'scrubbing uses no decryption or trading callbacks' do
    user = create(:user, otp_secret_key: 'KEPTOTPSECRET')
    exchange = create(:binance_exchange)
    key = create(:api_key, user: user, exchange: exchange)
    Bot.insert_all!([{ user_id: user.id, exchange_id: exchange.id, status: :scheduled, type: 'Bots::Signal' }])
    Rules::Withdrawal.insert_all!([{ user_id: user.id, exchange_id: exchange.id, status: :scheduled,
                                     type: 'Rules::Withdrawal', address: 'kept-address' }])
    ActiveRecord::Encryption.encryptor.expects(:decrypt).never
    Bot.any_instance.expects(:stop).never
    Rule.any_instance.expects(:stop).never

    Export::Portable.new.send(:scrub!)

    assert ApiKey.exists?(key.id)
    assert_equal 0, ApiKey.where.not(key: nil).count
    assert_equal 0, Bot.working.count
    assert_equal 0, Rule.working.count
  end

  test 'the AppConfig removal list covers the settings declared by the app' do
    keys = AppConfig.constants(false).filter_map do |name|
      value = AppConfig.const_get(name)
      value if value.is_a?(String) && value.match?(/\A(?:coingecko_api_key|market_data_|platform_connected_at|alpaca_|smtp_)/)
    end
    assert_equal keys.sort, Export::Portable::APP_CONFIG_KEYS.sort
    assert_equal %w[proxy_ smtp_], Export::Portable::APP_CONFIG_PREFIXES
  end

  test 'scrubbing clears a Rust-held engine lease without decrypting it' do
    AppConfig.set(EngineLease::KEY, { engine: 'rust', version: '0.1.0' }.to_json)
    ActiveRecord::Encryption.encryptor.expects(:decrypt).never

    Export::Portable.new.send(:scrub!)

    assert_not AppConfig.exists?(key: EngineLease::KEY)
    assert_nil EngineLease.check_handover!
  end

  test 'bot CSV paths stay short with a 150-character label and a large ID' do
    assert_bot_csv_path('a' * 150, id: 9_223_372_036_854_775_807)
  end

  test 'bot CSV labels are transliterated to ASCII and stripped of unsafe characters' do
    assert_equal 'csv/bots/1-creme-brulee.csv', assert_bot_csv_path('Crème _brûlée / 東京 🚀')
    assert_equal 'csv/bots/1-bot.csv', assert_bot_csv_path('東京 🚀')
  end

  test 'refuses ordinary app databases before migrating or scrubbing' do
    exporter = Export::Portable.new
    exporter.expects(:migrate!).never
    exporter.expects(:scrub!).never
    assert_raises(ExportScratch::UnsafeEnvironment) { exporter.call }
  end

  private

  def assert_bot_csv_path(label, id: 1)
    bot = Bot.new(id: id, label: label)
    bot.stubs(:orders_csv).returns("Order ID\nclosed-order\n")
    Bot.stubs(:find_each).yields(bot)
    Dir.mktmpdir do |dir|
      directory = Pathname.new(dir).join('csv')
      FileUtils.mkdir_p(directory.join('bots'))
      Export::Portable.new.send(:write_csv, directory)
      paths = Dir.glob('csv/bots/*', base: dir)
      assert_equal 1, paths.size
      path = paths.first
      assert_match(%r{\Acsv/bots/#{id}-[a-z0-9-]+\.csv\z}, path)
      assert_operator path.bytesize, :<=, 85
      assert_equal(['closed-order'], CSV.read(File.join(dir, path), headers: true).map { |row| row['Order ID'] })
      path
    end
  end
end
