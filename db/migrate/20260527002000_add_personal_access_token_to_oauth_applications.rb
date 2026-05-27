class AddPersonalAccessTokenToOauthApplications < ActiveRecord::Migration[8.1]
  def change
    add_column :oauth_applications, :personal_access_token, :boolean, default: false, null: false
    add_reference :oauth_applications, :personal_owner,
                  foreign_key: { to_table: :users }, null: true, index: false

    # DB-level guarantee: at most one personal app per user. The predicate
    # restricts uniqueness to rows that ARE personal-access-token apps, so
    # third-party DCR apps (which have NULL personal_owner_id) are
    # unconstrained.
    add_index :oauth_applications, :personal_owner_id,
              unique: true,
              where: 'personal_access_token = 1',
              name: 'index_oauth_applications_unique_personal_owner'
  end
end
