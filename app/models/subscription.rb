class Subscription < ApplicationRecord
  belongs_to :subscription_plan
  belongs_to :user

  scope :current, -> { where('end_time > ?', Time.now) }

  delegate :name, to: :subscription_plan
  delegate :display_name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan

  before_validation :set_sequence_number

  validates :sequence_number, presence: true, if: -> { legendary_badger? }
  validates :sequence_number,
            numericality: {
              only_integer: true,
              greater_than_or_equal_to: 0,
              less_than_or_equal_to: 999,
              message: '%<value>s is incorrect, please enter a number from 0 to 999'
            },
            if: -> { sequence_number.present? && legendary_badger? }
  validate do
    if sequence_number.present? && !legendary_badger?
      errors.add :sequence_number, :used, message: 'is only available for the Legendary Badger NFT plan'
    end
    errors.add :sequence_number, :used, message: '%<value>s is already used' if sequence_number_already_used?
  end

  private

  def sequence_number_already_used?
    legendary_badger? && sequence_number.present? && sequence_number.in?(Subscription.used_sequence_numbers - [sequence_number_was])
  end

  def legendary_badger?
    name == SubscriptionPlan::LEGENDARY_BADGER
  end

  def set_sequence_number
    self.sequence_number = next_sequence_number if legendary_badger? && !sequence_number.present?
  end

  def self.used_sequence_numbers
    legendary_badger_plan = SubscriptionPlan.find_by_name(SubscriptionPlan::LEGENDARY_BADGER)
    subscriptions = Subscription.current.where(subscription_plan_id: legendary_badger_plan.id)
    subscriptions.map(&:sequence_number).compact.sort
  end

  def next_sequence_number
    allowable_sequence_numbers = [*0..999] - self.class.used_sequence_numbers
    allowable_sequence_numbers.sample
  end
end
