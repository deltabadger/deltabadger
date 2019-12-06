class Payment < ApplicationRecord
  belongs_to :user

  enum currency: %i[USD EUR PLN]
  enum status: %i[unpaid pending paid confirmed failure cancelled]
end
