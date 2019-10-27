class Transaction < ApplicationRecord
  belongs_to :bot
  enum currency: %i[USD EUR PLN]
  enum status: %i[success failure]

  validates :bot, presence: true

  def get_errors
    JSON.parse(error_messages)
  end

  def price
    amount * rate
  end
end
