class SubscriptionPlan < ApplicationRecord
  SAVER = 'saver'.freeze
  INVESTOR = 'investor'.freeze
  HODLER = 'hodler'.freeze
  LEGENDARY_BADGER = 'legendary_badger'.freeze

  has_many :subscriptions

  validates :years, numericality: { only_integer: true, greater_than: 0 }
  validates :cost_eu, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_other, numericality: { greater_than_or_equal_to: 0 }
  validates :credits, numericality: { only_integer: true, greater_than: 0 }

  def duration
    years.years
  end

  def display_name
    I18n.t("subscriptions.#{name}")
  end
end
