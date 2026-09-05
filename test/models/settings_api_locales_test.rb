require 'test_helper'

# Repo convention: no default:, no EN-only strings — every locale carries a
# native translation (config/locales/CLAUDE.md).
class SettingsApiLocalesTest < ActiveSupport::TestCase
  KEYS = %w[
    settings.rest.token
    settings.rest.docs
    settings.rest.regenerate_token
    settings.mcp.tools_title
    settings.mcp.tools_description
    settings.mcp.connected_clients
    settings.mcp.client_permissions_hint
  ].freeze

  # Superseded by the switch states themselves: a half-set knob for "some", the column header
  # for the surface. Gone from every locale so nothing keeps translating dead copy.
  REMOVED = %w[
    settings.mcp.client_surface_rest
    settings.mcp.client_group_partial
  ].freeze

  test 'api settings keys exist in every available locale' do
    I18n.available_locales.each do |locale|
      KEYS.each do |key|
        # fallback: false — with config.i18n.fallbacks on, a plain exists? is
        # satisfied by the EN string and would never catch a missing locale.
        assert I18n.exists?(key, locale, fallback: false), "missing #{key} in #{locale}"
      end
    end
  end

  test 'retired api settings keys are gone from every locale' do
    I18n.available_locales.each do |locale|
      REMOVED.each do |key|
        assert_not I18n.exists?(key, locale, fallback: false), "#{key} still present in #{locale}"
      end
    end
  end
end
