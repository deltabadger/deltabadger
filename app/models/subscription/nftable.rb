module Subscription::Nftable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    before_validation :set_nft_id

    validates :nft_id, presence: true, if: :legendary?
    validates :nft_id, uniqueness: true, allow_nil: true
    validates :nft_id,
              numericality: {
                only_integer: true,
                in: 0..999,
                message: '%<value>s is incorrect, please enter a number from 0 to 999'
              },
              if: -> { nft_id.present? && legendary? }
    validate :nft_id_valid_for_legendary_badger
    validate :eth_address_is_valid, if: -> { eth_address.present? }

    def eth_address_is_valid?
      eth_address =~ Regexp.new(Ethereum.address_pattern)
    end

    def nft_name
      "Legendary Badger ##{nft_id}" if nft_id.present?
    end

    def nft_rarity
      LegendaryBadgersCollection::RARITIES[nft_id] if nft_id.present?
    end

    private

    def nft_id_valid_for_legendary_badger
      errors.add(:nft_id, :used, message: 'is only available for the Legendary Badger NFT plan') if nft_id.present? && !legendary?
    end

    def eth_address_is_valid
      errors.add(:eth_address, :invalid_address, eth_address: eth_address) unless eth_address_is_valid?
    end

    def set_nft_id
      self.nft_id = self.class.next_nft_id if legendary? && !nft_id.present?
    end
  end

  class_methods do
    def used_nft_ids
      subscriptions = by_plan_name(SubscriptionPlan::LEGENDARY_PLAN)
      subscriptions.map(&:nft_id).compact.sort
    end

    def claimed_nft_ids
      # we assume only Legendary Badger subscriptions can have an eth_address
      subscriptions = by_plan_name(SubscriptionPlan::LEGENDARY_PLAN).where.not(eth_address: nil)
      subscriptions.map(&:nft_id).compact.sort
    end

    def next_nft_id
      ([*0..999] - used_nft_ids).sample
    end
  end
end
