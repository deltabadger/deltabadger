class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  STATES = %i[created working stopped].freeze

  enum status: [*STATES]

  def currency
    settings.fetch('currency')
  end

  def buyer?
    settings.fetch('type') == 'buy'
  end

  def seller?
    !buyer?
  end
end
