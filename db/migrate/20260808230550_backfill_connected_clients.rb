# Records, for every connection that predates per-client permissions, a grant equal
# to what that connection could already do. Existing clients keep working; nothing
# they could not already do is granted.
#
# Self-contained on purpose: migration-local models and frozen tool lists. This is a
# one-time data transform that must keep describing THIS upgrade, so it must not
# call User, AppConfig or ConnectedClient — a historical migration that depends on
# head-of-branch code breaks fresh installs migrating from zero, and upgrades that
# skip versions.
class BackfillConnectedClients < ActiveRecord::Migration[8.1]
  # Verbatim copies of AppConfig::MCP_TOOL_DEFAULTS / REST_TOOL_DEFAULTS at the time
  # this migration was written. Do not update them if the constants change.
  MCP_DEFAULTS = {
    'list_bots' => true, 'get_bot_details' => true, 'list_exchanges' => true,
    'get_exchange_balances' => true, 'get_portfolio_summary' => true,
    'list_transactions' => true, 'create_bot' => false, 'start_bot' => false,
    'stop_bot' => false, 'update_bot_settings' => false, 'start_rule' => false,
    'stop_rule' => false, 'update_rule_settings' => false, 'list_open_orders' => true,
    'market_buy' => false, 'market_sell' => false, 'limit_buy' => false,
    'limit_sell' => false, 'cancel_order' => false, 'list_tax_jurisdictions' => true,
    'generate_tax_report' => true, 'get_tax_report_status' => true,
    'download_tax_report' => true, 'export_transactions_csv' => true,
    'list_account_transactions' => true
  }.freeze

  REST_DEFAULTS = {
    'list_bots' => false, 'get_bot_details' => false, 'list_exchanges' => false,
    'get_exchange_balances' => false, 'get_portfolio_summary' => false,
    'list_transactions' => false, 'list_open_orders' => false,
    'export_transactions_csv' => false, 'list_account_transactions' => false,
    'create_bot' => false, 'start_bot' => false, 'stop_bot' => false,
    'update_bot_settings' => false, 'start_rule' => false, 'stop_rule' => false,
    'update_rule_settings' => false, 'market_buy' => false, 'market_sell' => false,
    'limit_buy' => false, 'limit_sell' => false, 'cancel_order' => false
  }.freeze

  class Application < ActiveRecord::Base
    self.table_name = 'oauth_applications'
  end

  class Token < ActiveRecord::Base
    self.table_name = 'oauth_access_tokens'
  end

  class Grant < ActiveRecord::Base
    self.table_name = 'oauth_access_grants'
  end

  class Person < ActiveRecord::Base
    self.table_name = 'users'
  end

  class Client < ActiveRecord::Base
    self.table_name = 'connected_clients'
  end

  def up
    revoke_legacy_personal_app_credentials!

    created = 0

    scopes_by_pair.each do |(user_id, application_id), scopes|
      next if Client.exists?(user_id: user_id, oauth_application_id: application_id)

      Client.create!(
        user_id: user_id,
        oauth_application_id: application_id,
        mcp_tools: scopes.include?('mcp') ? enabled_for(user_id, :mcp) : [],
        rest_tools: scopes.include?('api') ? enabled_for(user_id, :rest) : []
      )
      created += 1
    end

    say "Recorded #{created} existing client connection(s)"
    created
  end

  def down
    # Nothing to undo: these records are the grants, and dropping them would
    # disconnect clients that were connected before this ran.
  end

  private

  # ToolAccess reads a token on a personal API application as the user acting as
  # themselves, and the backfill deliberately skips personal applications, so such a
  # token carries no grant record and appears in no connected-clients list.
  # allow_grant_flow_for_client now keeps these applications out of the authorization
  # flow, but that governs new credentials only — anything already issued has to be
  # retired here.
  #
  # The legitimate token is the one User#mint_personal_token! creates: `expires_in`
  # nil and no refresh token, because it is built with a plain `create!` rather than
  # through a grant flow. Anything on a personal application with either column set,
  # and every authorization code (there should never be one), came from the OAuth path.
  def revoke_legacy_personal_app_credentials!
    personal = Application.where(personal_access_token: true).pluck(:id)
    return if personal.empty?

    now = Time.current

    suspect = Token.where(application_id: personal, revoked_at: nil)
                   .where.not(refresh_token: nil)
                   .or(Token.where(application_id: personal, revoked_at: nil).where.not(expires_in: nil))
    count = suspect.update_all(revoked_at: now)
    count += Grant.where(application_id: personal, revoked_at: nil).update_all(revoked_at: now)

    say "Revoked #{count} OAuth credential(s) on personal API applications" if count.positive?
  end

  # One entry per (user, application), carrying the union of the scopes of every
  # live credential for that pair. Recording only the first credential would drop a
  # surface whenever a client holds separate mcp and api authorizations.
  def scopes_by_pair
    pairs = Hash.new { |h, k| h[k] = Set.new }

    live_credentials.each do |record|
      next if record.resource_owner_id.blank?
      next unless people.key?(record.resource_owner_id)
      # No database FK exists from credentials to applications, and connected_clients
      # has one — a dangling id would abort the migration during container boot.
      next unless application_ids.include?(record.application_id)

      pairs[[record.resource_owner_id, record.application_id]].merge(record.scopes.to_s.split)
    end

    pairs
  end

  # Access tokens: unrevoked, regardless of expiry — they expire hourly and refresh
  # indefinitely, and the refresh flow checks only revocation, so an expired access
  # token is the normal state of an idle connector.
  # Authorization codes: unrevoked AND unexpired — an expired code really is dead.
  def live_credentials
    personal = Application.where(personal_access_token: true).pluck(:id)

    tokens = Token.where(revoked_at: nil).where.not(application_id: personal).to_a
    grants = Grant.where(revoked_at: nil).where.not(application_id: personal).to_a
                  .reject { |g| g.created_at + g.expires_in.seconds < Time.current }

    tokens + grants
  end

  def application_ids
    @application_ids ||= Application.pluck(:id).to_set
  end

  def people
    @people ||= Person.pluck(:id, :mcp_settings, :rest_settings)
                      .to_h { |id, mcp, rest| [id, [mcp, rest]] }
  end

  # Mirrors User#mcp_tool_permissions / #rest_tool_permissions as they stood when
  # this migration was written: per-user overrides merged over the defaults.
  def enabled_for(user_id, surface)
    settings, defaults = if surface == :mcp
                           [people[user_id][0], MCP_DEFAULTS]
                         else
                           [people[user_id][1], REST_DEFAULTS]
                         end

    overrides = (settings || {})['tool_permissions'] || {}
    defaults.merge(overrides).select { |name, on| on && defaults.key?(name) }.keys
  end
end
