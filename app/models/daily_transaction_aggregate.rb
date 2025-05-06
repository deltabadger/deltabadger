class DailyTransactionAggregate < ApplicationRecord
  belongs_to :bot
  enum currency: %i[USD EUR PLN]
  enum status: %i[success failure skipped]

  before_create :round_numeric_fields

  validates :bot, presence: true

  scope :for_bot, ->(bot, limit: nil) { where(bot_id: bot.id).limit(limit).order(id: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }

  private

  def round_numeric_fields
    self.rate = rate&.round(18)
    self.amount = amount&.round(18)
    self.bot_price = bot_price&.round(18)
    self.total_amount = total_amount&.round(18)
    self.total_value = total_value&.round(18)
    self.total_invested = total_invested&.round(18)
  end
end
