require 'test_helper'

# What the user actually reads when a sync fails (issue #153).
class SyncKeyErrorFlashTest < ActionView::TestCase
  test 'a permission failure names the capability and does not tell the user to replace the key' do
    render_flash(reason: :permission, capability: :transactions)

    assert_includes rendered, 'transaction history'
    assert_includes rendered, 'missing the permission'
    assert_not_includes rendered, 'update the API key below'
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

  def render_flash(reason:, message: 'Some error', capability: :transactions)
    render partial: 'tracker/sync_key_error',
           locals: { exchange_name: 'Kraken', message: message, reason: reason, capability: capability }
  end
end
