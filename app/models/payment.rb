class Payment < ApplicationRecord
  belongs_to :user

  enum currency: %i[USD EUR PLN]
  # we only use unpaid, cancelled, paid
  enum status: %i[unpaid pending paid confirmed failure cancelled]
  enum commission_status: %i[commission_unexported commission_exported commission_paid]

  validates :first_name, :last_name, :birth_date, :user, presence: true
  validates_inclusion_of :eu, in: [true, false]

  scope :for_affiliate, -> (affiliate_id) {
    joins(:user).where(users: { referrer_id: affiliate_id })
  }
end
