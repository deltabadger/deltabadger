class Affiliate < ApplicationRecord
  belongs_to :user

  validates :first_name, :last_name, :birth_date, :user, :btc_address, :code, presence: true
  validates_inclusion_of :eu, in: [true, false]
end
