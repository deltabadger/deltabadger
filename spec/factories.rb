FactoryBot.define do
  factory :asset do
    portfolio { nil }
    ticker { 'MyString' }
    allocation { 1.5 }
  end

  factory :portfolio do
    user { nil }
    strategy { 1 }
    smart_allocation { 1 }
    benchmark { 1 }
    backtest_start { 'MyString' }
  end

  factory :payment do
    user
    subscription_plan { SubscriptionPlansRepository.new.saver }
    status { :paid }
    total { 12 }
    btc_total { 0.001 }
    btc_paid { 0.001 }
    currency { :EUR }
    first_name { Faker::Name.first_name }
    last_name { Faker::Name.last_name }
    birth_date { Faker::Date.birthday(min_age: 18, max_age: 123) }
    country { currency == :EUR ? 'Poland' : 'Other' }
    paid_at { created_at }
  end

  factory :user do
    email { Faker::Internet.email }
    password { Faker::Alphanumeric.alphanumeric(number: 10) }
    admin { false }
  end
end
