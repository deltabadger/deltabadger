FactoryBot.define do
  factory :affiliate do
    user
    type { :individual }
    btc_address { Faker::Blockchain::Bitcoin.address }
    code { Faker::Alphanumeric.unique.alphanumeric(number: 10).upcase }
    max_profit { 20 }
    discount_percent { 0.2 }
    total_bonus_percent { 0.2 }
    active { true }
  end

  factory :payment do
    user
    subscription_plan { SubscriptionPlansRepository.new.saver }
    status { :paid }
    total { 12 }
    crypto_total { 0.001 }
    crypto_paid { 0.001 }
    currency { :EUR }
    first_name { Faker::Name.first_name }
    last_name { Faker::Name.last_name }
    birth_date { Faker::Date.birthday(min_age: 18, max_age: 123) }
    eu { currency == :EUR }
    paid_at { created_at }
  end

  factory :user do
    email { Faker::Internet.email }
    password { Faker::Alphanumeric.alphanumeric(number: 10) }
    admin { false }
  end
end
