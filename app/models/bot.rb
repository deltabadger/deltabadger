class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  STATES = %i[created].freeze

  enum status: [*STATES]
end
