class Affiliate < ApplicationRecord
  belongs_to :user

  validates :first_name, :last_name, :birth_date, :user, :code, presence: true
  validates_inclusion_of :eu, in: [true, false]

  validate :btc_address, :valid_btc_address

  private

  def valid_btc_address
    return if ::Bitcoin.valid_address?(btc_address)

    errors.add(:btc_address, 'has to be valid')
  end
end
