module Bot::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :basic, -> { where(type: 'Bots::Basic') }
    scope :not_basic, -> { where.not(type: 'Bots::Basic') }

    scope :withdrawal, -> { where(type: 'Bots::Withdrawal') }
    scope :not_withdrawal, -> { where.not(type: 'Bots::Withdrawal') }

    scope :dca_single_asset, -> { where(type: 'Bots::DcaSingleAsset') }
    scope :not_dca_single_asset, -> { where.not(type: 'Bots::DcaSingleAsset') }

    scope :dca_dual_asset, -> { where(type: 'Bots::DcaDualAsset') }
    scope :not_dca_dual_asset, -> { where.not(type: 'Bots::DcaDualAsset') }

    scope :legacy, -> { where(type: %w[Bots::Basic Bots::Withdrawal]) }
    scope :not_legacy, -> { where.not(type: %w[Bots::Basic Bots::Withdrawal]) }
  end

  def basic?
    type == 'Bots::Basic'
  end

  def withdrawal?
    type == 'Bots::Withdrawal'
  end

  def dca_single_asset?
    type == 'Bots::DcaSingleAsset'
  end

  def dca_dual_asset?
    type == 'Bots::DcaDualAsset'
  end

  def legacy?
    basic? || withdrawal?
  end
end
