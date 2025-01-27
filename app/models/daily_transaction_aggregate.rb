class DailyTransactionAggregate < ApplicationRecord
  belongs_to :bot
  enum currency: %i[USD EUR PLN]
  enum status: %i[success failure skipped]

  validates :bot, presence: true

  scope :for_bot, ->(bot, limit: nil) { where(bot_id: bot.id).limit(limit).order(id: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }
end
