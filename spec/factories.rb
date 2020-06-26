FactoryBot.define do
  factory :affiliate do
    association :user, factory: :user
    first_name { Faker::Name.first_name }
    last_name { Faker::Name.last_name }
    birth_date { Faker::Date.birthday }
    eu { Faker::Boolean.boolean }
    btc_address { Faker::Blockchain::Bitcoin.address }
    code { Faker::Alphanumeric.alphanumeric(number: 12).upcase }
    max_profit { 500 }
    discount_percent { 0.1 }
    total_bonus_percent { 0.3 }
  end

  factory :user do
    email { Faker::Internet.email }
    password { Faker::Alphanumeric.alphanumeric(number: 10) }
    admin { false }
  end
end
