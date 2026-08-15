class AddLastSyncErrorToApiKeys < ActiveRecord::Migration[8.1]
  def change
    add_column :api_keys, :last_sync_error, :string
  end
end
