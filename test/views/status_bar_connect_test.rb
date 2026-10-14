# frozen_string_literal: true

require 'test_helper'

# A bot without a working key: the tile says Disconnected, the bot page offers Connect — never
# "Paused" next to a Connect button. The partial carries both; the stylesheets pick one per page.
class StatusBarConnectTest < ActionView::TestCase
  setup do
    def @controller.default_url_options = { locale: I18n.default_locale }

    # Dry run hands every bot a `correct` key, so a disconnected bot could never be rendered.
    Rails.configuration.dry_run = false
  end

  teardown { Rails.configuration.dry_run = true }

  test 'a paused bot without a working key shows Disconnected and Connect instead of Paused' do
    bot = create(:dca_single_asset, with_api_key: false, status: :stopped)
    render partial: 'bots/status/status_bar', locals: { bot: bot }

    assert_select 'a.bot-control__connect', text: I18n.t('button.connect')
    assert_select '.bot-control__status__disconnected', text: I18n.t('bot.status.disconnected')
    assert_not_includes rendered, I18n.t('bot.status.paused')
  end
end
