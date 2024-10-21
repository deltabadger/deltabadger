module Subscription::Legendary
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    before_validation :set_nft_id

    validates :nft_id, presence: true, if: :legendary_badger?
    validates :nft_id,
              numericality: {
                only_integer: true,
                in: 0..999,
                message: '%<value>s is incorrect, please enter a number from 0 to 999'
              },
              if: -> { nft_id.present? && legendary_badger? }
    validate :nft_id_valid_for_legendary_badger
    validate :nft_id_uniqueness
    validate :eth_address_is_valid, if: -> { eth_address.present? }

    def eth_address_is_valid?
      eth_address =~ Regexp.new(Ethereum.address_pattern)
    end

    private

    def nft_id_valid_for_legendary_badger
      errors.add(:nft_id, :used, message: 'is only available for the Legendary Badger NFT plan') if nft_id.present? && !legendary_badger? # rubocop:disable Layout/LineLength
    end

    def nft_id_uniqueness
      errors.add(:nft_id, :used, message: '%<value>s is already used') if nft_id_already_used?
    end

    def eth_address_is_valid
      errors.add(:eth_address, :invalid_address, eth_address: eth_address) unless eth_address_is_valid?
    end

    def nft_id_already_used?
      legendary_badger? && nft_id.present? && nft_id.in?(self.class.used_nft_ids - [nft_id_was])
    end

    def legendary_badger?
      name == SubscriptionPlan::LEGENDARY_PLAN
    end

    def set_nft_id
      self.class.nft_id = next_nft_id if legendary_badger? && !nft_id.present?
    end

    def next_nft_id
      ([*0..999] - self.class.used_nft_ids).sample
    end
  end

  class_methods do
    def used_nft_ids
      legendary_badger_plan = SubscriptionPlan.find_by_name(SubscriptionPlan::LEGENDARY_PLAN)
      subscriptions = current.where(subscription_plan_id: legendary_badger_plan.id)
      subscriptions.map(&:nft_id).compact.sort
    end

    def claimed_nft_ids
      # we assume only Legendary Badger subscriptions can have an eth_address
      subscriptions = current.where.not(eth_address: nil)
      subscriptions.map(&:nft_id).compact.sort
    end
  end
end
