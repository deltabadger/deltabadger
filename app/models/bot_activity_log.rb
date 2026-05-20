class BotActivityLog < ApplicationRecord
  belongs_to :bot

  enum :level, %i[info warning error]
end
