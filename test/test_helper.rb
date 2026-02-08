ENV['RAILS_ENV'] ||= 'test'
require_relative '../config/environment'
require 'rails/test_help'
require 'mocha/minitest'
require_relative 'support/exchange_mock_helpers'

puts "\n\e[1mDeltabadger v#{Rails.application.config.version}\e[0m\n\n"

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
