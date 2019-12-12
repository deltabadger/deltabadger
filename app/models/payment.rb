class Payment < ApplicationRecord
  belongs_to :user

  enum currency: %i[USD EUR PLN]
  enum status: %i[unpaid pending paid confirmed failure cancelled]

  validates :first_name, :last_name, :birth_date, :user, presence: true
  validates_inclusion_of :eu, in: [true, false]
end
