class BotSignal < ApplicationRecord
  belongs_to :bot

  enum :direction, { buy: 0, sell: 1 }
  enum :amount_type, { fixed: 0, percentage: 1 }

  validates :token, presence: true, uniqueness: true
  validates :direction, presence: true
  validates :amount, presence: true, numericality: { greater_than: 0 }

  before_validation :generate_token, on: :create

  def webhook_url
    "/hook/#{token}"
  end

  private

  def generate_token
    return if token.present?

    loop do
      self.token = SecureRandom.urlsafe_base64(4)
      break unless BotSignal.exists?(token: token)
    end
  end
end
