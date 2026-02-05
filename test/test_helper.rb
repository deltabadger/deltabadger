ENV['RAILS_ENV'] ||= 'test'
require_relative '../config/environment'
require 'rails/test_help'
require 'mocha/minitest'
require 'selenium/webdriver'
require_relative 'support/exchange_mock_helpers'

puts "\n\e[1mDeltabadger v#{Rails.application.config.version}\e[0m\n\n"

# Capybara configuration
Capybara.register_driver :headless_chrome do |app|
  options = Selenium::WebDriver::Chrome::Options.new
  options.add_argument('--headless=new')
  options.add_argument('--proxy-server=direct://')
  options.add_argument('--proxy-bypass-list=*')

  Capybara::Selenium::Driver.new(app, browser: :chrome, options: options)
end

Capybara.javascript_driver = :headless_chrome

module ActiveSupport
  class TestCase
    include FactoryBot::Syntax::Methods
    include ExchangeMockHelpers

    # Run tests in parallel with specified workers
    parallelize(workers: :number_of_processors)

    # Setup all fixtures in test/fixtures/*.yml for all tests in alphabetical order.
    self.use_transactional_tests = true
  end
end

module ActionDispatch
  class IntegrationTest
    include Devise::Test::IntegrationHelpers
  end
end

module DeviseHelpers
  def sign_in_user
    user = User.create(
      email: 'test@test.com',
      password: 'password',
      password_confirmation: 'password',
      confirmed_at: Time.now
    )
    sign_in user
    user
  end
end
