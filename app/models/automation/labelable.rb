module Automation::Labelable
  extend ActiveSupport::Concern

  included do
    validates :label, presence: true

    # Named at save time, not at build time: the name comes from the settings, and the wizard
    # fills those in one step at a time on an in-memory bot.
    before_validation :set_default_label, if: -> { label.blank? }
    after_find :ensure_label_exists
  end

  # What the bot holds, said the way the user would say it. Each type overrides this; nil means
  # there is nothing to read a name from (a settings-less bot, an asset row that went away).
  def default_label
    nil
  end

  private

  def set_default_label
    self.label = generate_label
  end

  def ensure_label_exists
    return unless label.blank?

    update!(label: generate_label)
  end

  def generate_label
    default_label.presence || I18n.t('bot.new')
  end

  # "BTC, ETH, XRP + 3" — a basket is named after its first few holdings, then how many are left.
  def basket_label(*asset_ids)
    by_id = Asset.where(id: asset_ids.compact).index_by { |asset| asset.id.to_s }
    symbols = asset_ids.filter_map { |id| by_id[id.to_s]&.symbol }
    rest = symbols.size - 3

    rest.positive? ? "#{symbols.first(3).join(', ')} + #{rest}" : symbols.join(', ')
  end
end
