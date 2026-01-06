module Bot::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :dca_single_asset, -> { where(type: 'Bots::DcaSingleAsset') }
    scope :not_dca_single_asset, -> { where.not(type: 'Bots::DcaSingleAsset') }

    scope :dca_dual_asset, -> { where(type: 'Bots::DcaDualAsset') }
    scope :not_dca_dual_asset, -> { where.not(type: 'Bots::DcaDualAsset') }
  end

  def dca_single_asset?
    type == 'Bots::DcaSingleAsset'
  end

  def dca_dual_asset?
    type == 'Bots::DcaDualAsset'
  end
end
