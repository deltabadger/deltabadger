class Card < ApplicationRecord
  belongs_to :user

  validates :token, presence: true, uniqueness: { scope: :user_id }
  validates :ip, format: { with: /\A\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\z/ }
end
