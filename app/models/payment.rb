class Payment < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan

  enum currency: %i[USD EUR PLN]
  # we only use unpaid, cancelled, paid
  enum status: %i[unpaid pending paid confirmed failure cancelled]
  enum payment_type: %i[bitcoin wire stripe zen]
  validates :user, presence: true
  validates :birth_date, presence: true, if: :bitcoin?

  def bitcoin?
    payment_type == 'bitcoin'
  end

  def eu?
    country != VatRate::NOT_EU
  end
end
