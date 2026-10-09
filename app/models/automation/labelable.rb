module Automation::Labelable
  extend ActiveSupport::Concern

  included do
    validates :label, presence: true

    # Named at save time, not at build time: the name comes from the settings, and the wizard
    # fills those in one step at a time on an in-memory bot.
    before_validation :set_default_label, if: -> { label.blank? }
    after_find :ensure_label_exists
    after_save { @label_unsaved = false }
  end

  # What the bot holds, said the way the user would say it. Each type overrides this; nil means
  # there is nothing to read a name from (a settings-less bot, an asset row that went away).
  def default_label
    nil
  end

  # Assigning the shown name of a bot loaded without one is naming it on purpose: the value equals
  # what is in memory, so mark it changed or the save would leave the row unnamed.
  def label=(value)
    super
    label_will_change! if @label_unsaved && value.present?
  end

  private

  def set_default_label
    self.label = generate_label
  end

  # Loading is a read: a missing name is computed for display and never saved from here, so a
  # GET (the page, MCP get_bot / list_bots) leaves the row and its updated_at untouched. The name
  # is written only when something saves the bot on purpose (set_default_label above).
  def ensure_label_exists
    return unless label.blank?

    self.label = generate_label
    clear_attribute_changes([:label])
    @label_unsaved = true
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
