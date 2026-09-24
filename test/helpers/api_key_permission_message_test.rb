require 'test_helper'

# When a key check names permissions, the message has to name them in the words of the steps shown
# right under it — or the user is left matching two vocabularies.
class ApiKeyPermissionMessageTest < ActionView::TestCase
  include BotHelper

  setup do
    @kraken = create(:kraken_exchange)
  end

  def key(type, missing: [], forbidden: [])
    api_key = build(:api_key, exchange: @kraken, key_type: type)
    api_key.stubs(:missing_permissions).returns(missing)
    api_key.stubs(:forbidden_permissions).returns(forbidden)
    api_key
  end

  test 'names what is missing and what to turn off, then says what to do' do
    message = api_key_permission_message(key(:trading, missing: %w[query-ledger], forbidden: %w[withdraw-funds]))

    assert_equal 'This key is missing: Data → Query ledger entries. ' \
                 'For your safety, turn off: Funds permissions → Withdraw. ' \
                 'Update the key on Kraken, or create a new one following the steps below.', message
  end

  test 'a missing-only and a forbidden-only message carry just their own sentence' do
    missing = api_key_permission_message(key(:trading, missing: %w[query-funds close-trades]))
    assert_includes missing, 'Funds permissions → Query, Order and Trades → Cancel & close orders'
    assert_not_includes missing, 'turn off'

    forbidden = api_key_permission_message(key(:withdrawal, forbidden: %w[modify-trades]))
    assert_includes forbidden, 'turn off: Order and Trades → Create & modify orders'
    assert_not_includes forbidden, 'missing'
  end

  # German trading steps are German; German read-only and withdrawal steps fall back to English.
  test 'labels follow the language the steps render in' do
    I18n.with_locale(:de) do
      assert_includes api_key_permission_message(key(:trading, missing: %w[query-ledger])),
                      'Daten → Hauptbucheinträge abfragen'
      assert_includes api_key_permission_message(key(:read_only, missing: %w[query-ledger])),
                      'Data → Query ledger entries'
      assert_includes api_key_permission_message(key(:withdrawal, missing: %w[withdraw-funds])),
                      'Funds permissions → Withdraw'
    end
  end

  # The guarantee itself: for every locale and key type, each permission the check can name appears
  # in the steps rendered for that key type in that locale — the group always, and the permission's
  # own name wherever the steps ask for it (a forbidden permission may be named only by its group,
  # as in "do not enable any Order and Trades permission").
  test 'every permission the check can name appears in the steps shown with it' do
    Exchanges::Kraken::KEY_PERMISSIONS.each do |type, rules|
      I18n.available_locales.each do |locale|
        I18n.with_locale(locale) do
          steps = CGI.unescapeHTML(strip_tags(render_api_key_instructions(key(type)))).squish
          (rules[:required] + rules[:forbidden]).each do |flag|
            group, name = api_key_permission_label(@kraken, type, flag).split(' → ')
            assert_includes steps, group, "#{locale} #{type} #{flag}: group"
            next unless rules[:required].include?(flag) || flag == 'withdraw-funds'

            assert_includes steps, name, "#{locale} #{type} #{flag}: name"
          end
        end
      end
    end
  end

  test 'the Kraken reading steps ask for funds query and the ledger, and no order permission' do
    steps = CGI.unescapeHTML(strip_tags(render_read_api_key_instructions(@kraken))).squish

    assert_includes steps, 'Query ledger entries'
    assert_not_includes steps, 'Create & modify orders'
    assert_not_includes steps, 'Cancel & close orders'
  end
end
