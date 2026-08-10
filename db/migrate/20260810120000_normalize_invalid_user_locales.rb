# users.locale had no validation until 2026-08-06, and settings#update_locale wrote the raw
# param straight into it — so a row can hold "" or anything else that was ever POSTed. "" is
# now normalised away on write and the inclusion validator refuses the rest, which would wedge
# any such row: every later save, including an unrelated name or password change, would fail on
# an attribute the user never touched. Reset them to NULL, which is what "no preference" has
# always meant to every reader of the column.
#
# Self-contained on purpose (see BackfillConnectedClients): a migration-local model, and the
# locale list frozen as it stood here rather than read from I18n — a historical migration that
# tracks head-of-branch config breaks fresh installs and version-skipping upgrades.
class NormalizeInvalidUserLocales < ActiveRecord::Migration[8.1]
  # Verbatim copy of config.i18n.available_locales at the time this was written.
  # Do not update it if that list changes.
  LOCALES = %w[en pl es de nl fr pt ru it bg el sv da cs sk].freeze

  class Person < ActiveRecord::Base
    self.table_name = 'users'
  end

  def up
    # The `locale: nil` clause is redundant under SQL's NOT IN, which already drops NULLs —
    # it is here so that reading this migration does not require knowing that.
    cleared = Person.where.not(locale: nil).where.not(locale: LOCALES).update_all(locale: nil)
    say "Cleared #{cleared} unusable user locale(s)" if cleared.positive?
    cleared
  end

  def down
    # Nothing to undo: the values this cleared were ones no reader of the column could use.
  end
end
