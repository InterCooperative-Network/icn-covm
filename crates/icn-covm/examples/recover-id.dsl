# Guardian-based DID Recovery
# This DSL program defines a mechanism for recovering access to a DID
# through a social recovery process requiring approval from trusted guardians
# 
# Usage:
# - Setup: Configure guardians with threshold
# - Recovery: Initiate recovery request with new public key
# - Vote: Guardians vote to approve or deny the recovery
# - Finalize: Apply recovery after threshold is met

# Process configuration parameters
PARAM recovery_request_id          # Unique identifier for this recovery request
PARAM recovery_threshold 0.67      # Default threshold (percentage as decimal) of guardians needed
PARAM recovery_timeout_days 14     # Default expiration for recovery requests

# Identity parameters
PARAM identity_did                 # DID to recover
PARAM new_public_key               # New public key to associate with the DID
PARAM recovery_proof               # Proof of identity (e.g. answers to security questions)

# Namespace for recovery data
DEFINE recovery_namespace "recovery"

# Recovery state enum values
DEFINE STATE_PENDING 1
DEFINE STATE_APPROVED 2
DEFINE STATE_REJECTED 3
DEFINE STATE_EXPIRED 4
DEFINE STATE_COMPLETED 5

# Function to list guardians for a DID
function list_guardians(did)
  # Retrieve the guardians list from storage
  load did + "/guardians" recovery_namespace -> guardians_json
  
  # If no guardians are set up, return empty list
  if guardians_json == null then
    return []
  end
  
  # Return the list of guardians
  return parse_json(guardians_json)
end

# Function to get the total number of guardians
function count_guardians(did) 
  list_guardians(did) -> guardians
  return length(guardians)
end

# Function to setup guardians for a DID
function setup_guardians(did, guardian_dids, custom_threshold)
  # Ensure only the DID owner can add guardians
  require_ownership(did)
  
  # Validate custom threshold if provided
  if custom_threshold != null then
    if custom_threshold < 0.5 || custom_threshold > 1.0 then
      fail "Threshold must be between 0.5 and 1.0"
    end
    recovery_threshold = custom_threshold
  end
  
  # Ensure at least 3 guardians for security
  if length(guardian_dids) < 3 then
    fail "At least 3 guardians are required for security"
  end
  
  # Store guardian list and threshold
  guardian_data = {
    "dids": guardian_dids,
    "threshold": recovery_threshold,
    "updated_at": now()
  }
  
  store did + "/guardians" to_json(guardian_data) recovery_namespace
  
  # Log guardian setup
  log "Guardians setup for " + did + " with threshold " + recovery_threshold
  
  return true
end

# Function to initiate a recovery request
function initiate_recovery(did, new_key, proof)
  # Create recovery request ID if not provided
  if recovery_request_id == null then
    recovery_request_id = did + "/recovery/" + random_id(16)
  end
  
  # Get guardian configuration
  list_guardians(did) -> guardians
  if length(guardians) == 0 then
    fail "No guardians configured for this DID"
  end
  
  # Calculate expiration time
  expiration = now() + (recovery_timeout_days * 24 * 60 * 60)
  
  # Create recovery request
  recovery_request = {
    "did": did,
    "new_public_key": new_key,
    "proof": proof,
    "state": STATE_PENDING,
    "requested_at": now(),
    "expires_at": expiration,
    "guardian_approvals": {},
    "guardian_rejections": {}
  }
  
  # Store recovery request
  store recovery_request_id to_json(recovery_request) recovery_namespace
  
  # Notify guardians (would integrate with notification system)
  foreach guardian in guardians do
    log "Notifying guardian " + guardian + " of recovery request " + recovery_request_id
    # notify_guardian(guardian, recovery_request_id, did)
  end
  
  # Log recovery initiation
  log "Recovery initiated for " + did + " with request ID " + recovery_request_id
  
  return recovery_request_id
end

# Function for a guardian to vote on a recovery request
function guardian_vote(request_id, guardian_did, approve)
  # Load the recovery request
  load request_id recovery_namespace -> request_json
  if request_json == null then
    fail "Recovery request not found"
  end
  
  request = parse_json(request_json)
  
  # Check if request is still pending
  if request.state != STATE_PENDING then
    fail "Recovery request is no longer pending"
  end
  
  # Check if request has expired
  if now() > request.expires_at then
    request.state = STATE_EXPIRED
    store request_id to_json(request) recovery_namespace
    fail "Recovery request has expired"
  end
  
  # Verify the voter is a guardian
  list_guardians(request.did) -> guardians
  is_guardian = false
  foreach guardian in guardians do
    if guardian == guardian_did then
      is_guardian = true
      break
    end
  end
  
  if !is_guardian then
    fail "Voter is not a guardian for this DID"
  end
  
  # Record the vote
  if approve then
    request.guardian_approvals[guardian_did] = now()
    log "Guardian " + guardian_did + " approved recovery request " + request_id
  else
    request.guardian_rejections[guardian_did] = now()
    log "Guardian " + guardian_did + " rejected recovery request " + request_id
  end
  
  # Update the request in storage
  store request_id to_json(request) recovery_namespace
  
  # Check if threshold is reached for approval or rejection
  approval_count = count_keys(request.guardian_approvals)
  rejection_count = count_keys(request.guardian_rejections)
  total_guardians = length(guardians)
  
  # Get threshold from guardian configuration
  load request.did + "/guardians" recovery_namespace -> guardian_config_json
  guardian_config = parse_json(guardian_config_json)
  threshold_pct = guardian_config.threshold
  
  # Calculate threshold number of guardians
  threshold_count = ceil(total_guardians * threshold_pct)
  
  # Check if approval threshold is met
  if approval_count >= threshold_count then
    request.state = STATE_APPROVED
    store request_id to_json(request) recovery_namespace
    log "Recovery request " + request_id + " approved by " + approval_count + " guardians"
  # Check if rejection threshold is met (more than 1/3 reject)
  elif rejection_count > total_guardians - threshold_count then
    request.state = STATE_REJECTED
    store request_id to_json(request) recovery_namespace
    log "Recovery request " + request_id + " rejected by " + rejection_count + " guardians"
  end
  
  return true
end

# Function to finalize an approved recovery
function finalize_recovery(request_id)
  # Load the recovery request
  load request_id recovery_namespace -> request_json
  if request_json == null then
    fail "Recovery request not found"
  end
  
  request = parse_json(request_json)
  
  # Check if request is approved
  if request.state != STATE_APPROVED then
    fail "Recovery request is not approved"
  end
  
  # Perform the key update
  did = request.did
  new_key = request.new_public_key
  
  # Call to the identity system to replace the key
  success = replace_did_key(did, new_key)
  
  if success then
    # Update request state
    request.state = STATE_COMPLETED
    request.completed_at = now()
    store request_id to_json(request) recovery_namespace
    
    # Log recovery completion
    log "Recovery completed for " + did + " with new public key"
  else
    fail "Failed to update public key for " + did
  end
  
  return success
end

# Function to check recovery request status
function check_recovery_status(request_id)
  # Load the recovery request
  load request_id recovery_namespace -> request_json
  if request_json == null then
    return {"error": "Recovery request not found"}
  end
  
  request = parse_json(request_json)
  
  # Check if request has expired but still marked pending
  if request.state == STATE_PENDING && now() > request.expires_at then
    request.state = STATE_EXPIRED
    store request_id to_json(request) recovery_namespace
  end
  
  # Get the number of approvals and rejections
  approval_count = count_keys(request.guardian_approvals)
  rejection_count = count_keys(request.guardian_rejections)
  
  # Get the total number of guardians
  list_guardians(request.did) -> guardians
  total_guardians = length(guardians)
  
  # Return the status details
  return {
    "request_id": request_id,
    "did": request.did,
    "state": request.state,
    "state_label": state_to_label(request.state),
    "requested_at": request.requested_at,
    "expires_at": request.expires_at,
    "completed_at": request.completed_at,
    "approvals": approval_count,
    "rejections": rejection_count,
    "total_guardians": total_guardians
  }
end

# Helper function to convert state code to label
function state_to_label(state)
  if state == STATE_PENDING then return "pending"
  elif state == STATE_APPROVED then return "approved"
  elif state == STATE_REJECTED then return "rejected"
  elif state == STATE_EXPIRED then return "expired"
  elif state == STATE_COMPLETED then return "completed"
  else return "unknown"
  end
end

# Main program entry point - determine which function to call based on action param
PARAM action
PARAM guardian_did

if action == "setup" then
  PARAM guardian_dids
  PARAM custom_threshold
  
  setup_guardians(identity_did, guardian_dids, custom_threshold)
  
elif action == "initiate" then
  initiate_recovery(identity_did, new_public_key, recovery_proof)
  
elif action == "vote" then
  PARAM approve
  
  guardian_vote(recovery_request_id, guardian_did, approve)
  
elif action == "finalize" then
  finalize_recovery(recovery_request_id)
  
elif action == "status" then
  check_recovery_status(recovery_request_id)
  
else
  fail "Unknown action: " + action
end 