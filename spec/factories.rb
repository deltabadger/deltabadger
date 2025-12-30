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

  factory :user do
    email { Faker::Internet.email }
    password { Faker::Alphanumeric.alphanumeric(number: 10) }
    admin { false }
  end
end
