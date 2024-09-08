class Subscription < ApplicationRecord
  belongs_to :subscription_plan
  belongs_to :user

  scope :current, -> { where('end_time > ?', Time.now) }

  delegate :name, to: :subscription_plan
  delegate :display_name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan

  before_validation :set_sequence_number

  validates :sequence_number, presence: true, if: -> { legendary? }
  validates :sequence_number,
            numericality: {
              only_integer: true,
              greater_than_or_equal_to: 0,
              less_than_or_equal_to: 999,
              message: '%<value>s is incorrect, please enter a number from 0 to 999'
            },
            if: -> { sequence_number.present? && legendary? }
  validate do
    if sequence_number.present? && !legendary?
      errors.add :sequence_number, :used, message: 'is only available for the Legendary plan'
    end
    errors.add :sequence_number, :used, message: '%<value>s is already used' if sequence_number_already_used?
  end

  validate :eth_address_is_valid, if: -> { !eth_address.nil? }

  private

  def sequence_number_already_used?
    legendary? && sequence_number.present? && sequence_number.in?(Subscription.used_sequence_numbers - [sequence_number_was])
  end

  def legendary?
    name == SubscriptionPlan::LEGENDARY_PLAN
  end

  def set_sequence_number
    self.sequence_number = next_sequence_number if legendary? && !sequence_number.present?
  end

  def self.used_sequence_numbers
    legendary_plan = SubscriptionPlan.find_by_name(SubscriptionPlan::LEGENDARY_PLAN)
    subscriptions = Subscription.current.where(subscription_plan_id: legendary_plan.id)
    subscriptions.map(&:sequence_number).compact.sort
  end

  def next_sequence_number
    allowable_sequence_numbers = [*0..999] - self.class.used_sequence_numbers
    allowable_sequence_numbers.sample
  end

  def eth_address_is_valid
    return if eth_address =~ /^0x[a-fA-F0-9]{40}$/

    errors.add(:eth_address, 'is not a valid Ethereum address')
  end
end
