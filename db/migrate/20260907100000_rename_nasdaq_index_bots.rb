class RenameNasdaqIndexBots < ActiveRecord::Migration[8.1]
  def up
    select_rows("SELECT id, settings, label FROM bots WHERE type = 'Bots::DcaIndex'").each do |id, settings_json, label|
      settings = begin
        JSON.parse(settings_json.to_s)
      rescue StandardError
        {}
      end
      next unless settings['index_category_id'] == 'nasdaq-100'

      settings['index_name_prefix'] = 'ND'
      settings['index_name'] = 'ND100'
      # Existing bots keep the count they were saved with (Decision 17): the user chose it.
      settings['hold_all'] = false unless settings.key?('hold_all')
      new_label = label.to_s.sub(/\ANasdaq (\d+)\z/) { "ND#{Regexp.last_match(1)}" }
      execute ActiveRecord::Base.send(:sanitize_sql_array,
                                      ['UPDATE bots SET settings = ?, label = ? WHERE id = ?', settings.to_json, new_label, id])
    end
  end

  def down; end
end
