require 'test_helper'

# Repo convention: no default:, no EN-only strings — every locale carries a native translation
# (config/locales/CLAUDE.md).
class SyncFailureLocalesTest < ActiveSupport::TestCase
  KEYS = %w[
    tracker.sync_permission_error_html
    tracker.sync_transient_error_html
    tracker.sync_generic_error_html
    tracker.capabilities.transactions
    tracker.capabilities.balances
  ].freeze

  test 'sync failure keys exist in every available locale' do
    I18n.available_locales.each do |locale|
      KEYS.each do |key|
        # fallback: false — with config.i18n.fallbacks on, a plain exists? is satisfied by the EN
        # string and would never catch a missing locale.
        assert I18n.exists?(key, locale, fallback: false), "missing #{key} in #{locale}"
      end
    end
  end

  test 'every locale renders real copy, not the interpolation placeholders' do
    I18n.available_locales.each do |locale|
      rendered = I18n.t('tracker.sync_permission_error_html', locale: locale, exchange: 'Kraken',
                                                              capability: I18n.t('tracker.capabilities.transactions', locale: locale))
      assert_includes rendered, 'Kraken', "#{locale}: exchange not interpolated"
      assert_not_includes rendered, '%{', "#{locale}: unresolved interpolation"
    end
  end

  # The instruction list is the only place naming the exact Kraken checkbox the tracker needs, and
  # its absence is what produced issue #153. A translation sweep must not quietly drop it again.
  test 'every locale asks Kraken users for the ledger permission' do
    I18n.available_locales.each do |locale|
      flattened = I18n.t('bot.api.kraken.instructions', locale: locale, fallback: false).to_s
      assert_includes flattened.downcase, 'ledger', "#{locale}: Kraken key setup never mentions the ledger permission"
      assert_not_includes flattened, 'will not work if you add more permissions',
                          "#{locale}: the warning that told users NOT to add it is back"
    end
  end

  # Structure, not just presence. A locale that has drifted out of shape — a group nested one level
  # too shallow, an item mangled by a bulk edit — still passes a keyword check while showing the
  # user a broken checklist. Both failures have happened in this block (the Spanish Funds group had
  # lost its child; a French line kept the tail of a deleted sentence).
  test 'every locale states the Kraken permissions in the same shape as English' do
    english = instruction_shape(I18n.t('bot.api.kraken.instructions', locale: :en))

    I18n.available_locales.each do |locale|
      instructions = I18n.t('bot.api.kraken.instructions', locale: locale, fallback: false)
      assert_equal english, instruction_shape(instructions), "#{locale}: Kraken instructions drifted"

      each_instruction(instructions) do |text|
        refute_match(%r{</b>['"]}, text, "#{locale}: leftover fragment after a permission label")
      end
    end
  end

  private

  # Bold labels per item plus nesting — enough to catch a lost or misplaced entry, blind to wording.
  def instruction_shape(list)
    Array(list).map { |item| [item[:text_html].to_s.scan('<b>').size, instruction_shape(item[:sub_instructions])] }
  end

  def each_instruction(list, &block)
    Array(list).each do |item|
      block.call(item[:text_html].to_s)
      each_instruction(item[:sub_instructions], &block)
    end
  end
end
