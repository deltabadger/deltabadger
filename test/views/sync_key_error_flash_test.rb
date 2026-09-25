require 'test_helper'

# What the user actually reads when a sync fails (issue #153).
class SyncKeyErrorFlashTest < ActionView::TestCase
  test 'a permission failure names the capability and does not tell the user to replace the key' do
    render_flash(reason: :permission, capability: :transactions)

    assert_includes rendered, 'transaction history'
    assert_includes rendered, 'missing the permission'
    assert_not_includes rendered, 'update the API key below'
  end

  # The key is still :correct, so nothing else on the page offers a way out.
  test 'a permission failure on the trading key offers both fixes' do
    render_flash(reason: :permission, capability: :transactions)

    assert_select 'a.rbutton[href=?]', new_tracker_add_api_key_path(exchange_id: 7, key_type: 'trading')
    assert_select 'a.rbutton[href=?]', new_tracker_add_api_key_path(exchange_id: 7, key_type: 'read_only')
  end

  test 'a permission failure on the tracker key offers to replace it' do
    render_flash(reason: :permission, capability: :transactions, key_type: 'read_only')

    assert_select 'a.rbutton', count: 1
    assert_select 'a.rbutton[href=?]', new_tracker_add_api_key_path(exchange_id: 7, key_type: 'read_only')
  end

  test 'no other failure offers the key form' do
    %i[invalid transient failed].each do |reason|
      render_flash(reason: reason)

      assert_select 'a.rbutton', count: 0
    end
  end

  test 'the balances capability is named for the other caller' do
    render_flash(reason: :permission, capability: :balances)

    assert_includes rendered, 'balances'
  end

  test 'an invalid key is the only failure that tells the user to replace it' do
    render_flash(reason: :invalid)

    assert_includes rendered, 'update the API key below'
  end

  test 'a transient failure advises retrying, not replacing the key' do
    render_flash(reason: :transient)

    assert_includes rendered, 'temporarily unavailable'
    assert_not_includes rendered, 'update the API key below'
  end

  test 'an unclassified failure shows the message without key advice' do
    render_flash(reason: :failed, message: 'EGeneral:Something new')

    assert_includes rendered, 'EGeneral:Something new'
    assert_not_includes rendered, 'update the API key below'
  end

  # The message is exchange-controlled. The _html key suffix makes the translate helper escape
  # interpolations; a stray .html_safe here would hand any venue an injection point.
  test 'an exchange error carrying markup is escaped' do
    render_flash(reason: :failed, message: '<script>alert(1)</script>')

    assert_not_includes rendered, '<script>'
    assert_includes rendered, '&lt;script&gt;'
  end

  private

  def render_flash(reason:, message: 'Some error', capability: :transactions, key_type: 'trading')
    render partial: 'tracker/sync_key_error',
           locals: { exchange_name: 'Kraken', exchange_id: 7, key_type: key_type, message: message, reason: reason,
                     capability: capability }
  end
end
