class Subscription < ApplicationRecord
  belongs_to :subscription_plan
  belongs_to :user

  scope :current, -> { where('end_time > ?', Time.now) }

  delegate :name, to: :subscription_plan
  delegate :display_name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan

  before_validation :set_nft_id

  validates :nft_id, presence: true, if: -> { legendary_badger? }
  validates :nft_id,
            numericality: {
              only_integer: true,
              greater_than_or_equal_to: 0,
              less_than_or_equal_to: 999,
              message: '%<value>s is incorrect, please enter a number from 0 to 999'
            },
            if: -> { nft_id.present? && legendary_badger? }
  validate do
    if nft_id.present? && !legendary_badger?
      errors.add :nft_id, :used, message: 'is only available for the Legendary Badger NFT plan'
    end
    errors.add :nft_id, :used, message: '%<value>s is already used' if nft_id_already_used?
  end

  validate :eth_address_is_valid, if: -> { !eth_address.nil? }

  private

  def nft_id_already_used?
    legendary_badger? && nft_id.present? && nft_id.in?(Subscription.used_nft_ids - [nft_id_was])
  end

  def legendary_badger?
    name == SubscriptionPlan::LEGENDARY_BADGER
  end

  def set_nft_id
    self.nft_id = next_nft_id if legendary_badger? && !nft_id.present?
  end

  def self.used_nft_ids
    legendary_badger_plan = SubscriptionPlan.find_by_name(SubscriptionPlan::LEGENDARY_BADGER)
    subscriptions = Subscription.current.where(subscription_plan_id: legendary_badger_plan.id)
    subscriptions.map(&:nft_id).compact.sort
  end

  def self.claimed_nft_ids
    legendary_badger_plan = SubscriptionPlan.find_by_name(SubscriptionPlan::LEGENDARY_BADGER)
    subscriptions = Subscription.current.where(subscription_plan_id: legendary_badger_plan.id).where.not(eth_address: nil)
    subscriptions.map(&:nft_id).compact.sort
  end

  def next_nft_id
    allowable_nft_ids = [*0..999] - self.class.used_nft_ids
    allowable_nft_ids.sample
  end

  def eth_address_is_valid
    return if eth_address =~ /^0x[a-fA-F0-9]{40}$/

    errors.add(:eth_address, 'is not a valid Ethereum address')
  end
end
