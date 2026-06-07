import argparse
import json
from dataclasses import dataclass, asdict
from enum import Enum

# --- 1. Define our Data Structures ---
class FaultPolicy(Enum):
    """Enumerations help prevent 'magic strings' and make code type-safe."""
    NONE = "None"
    TORN_WRITE = "TornWrite"
    LOST_WRITE = "LostWrite"
    UNKNOWN = "Unknown" # Explicitly track unrecognized fault types

@dataclass
class TestRecord:
    test_name: str
    active_policy: FaultPolicy
    workload_status: str

# --- 2. Define the Engine Logic ---
def parse_log_file(file_path: str) -> list[TestRecord]:
    """Parses a chaos_rs log file and returns a list of structured records."""
    records = []
    current_test = None

    # Using an explicit encoding ensures the file is read correctly across different OS environments
    with open(file_path, 'r', encoding="utf-8") as file:
        for line in file:
            clean_line = line.strip()
            if not clean_line:
                continue

            if clean_line.startswith("[RUNNING]"):
                if current_test is not None:
                    # Construct our clean Dataclass instance before storing it
                    records.append(TestRecord(
                        test_name=current_test["name"],
                        active_policy=current_test["policy"],
                        workload_status=current_test["status"]
                    ))
                
                current_test = {
                    "name": clean_line.replace("[RUNNING]", "").strip(),
                    "policy": FaultPolicy.NONE,
                    "status": "SUCCESS"
                }

            elif clean_line.startswith("[FAULT_ARMED]") and current_test is not None:
                policy_text = clean_line.split(":")[-1].strip()
                try:
                    current_test["policy"] = FaultPolicy(policy_text)
                except ValueError:
                    current_test["policy"] = FaultPolicy.UNKNOWN

            elif clean_line.startswith("[WORKLOAD_RESULT]") and current_test is not None:
                if "Error:" in clean_line:
                    current_test["status"] = clean_line.split("Error:")[-1].strip()

        # Catch the final item
        if current_test is not None:
            records.append(TestRecord(
                test_name=current_test["name"],
                active_policy=current_test["policy"],
                workload_status=current_test["status"]
            ))

    return records

def summarize(records: list[TestRecord]) -> dict[str, int]:
    """Processes the records to generate high-level test statistics."""
    total = len(records)
    successful = sum(1 for r in records if r.workload_status == "SUCCESS")
    return {
        "Total": total,
        "Successful": successful,
        "Failed": total - successful,
        "Faulted": sum(
            1 for r in records 
            if r.active_policy not in (FaultPolicy.NONE, FaultPolicy.UNKNOWN)
        )
    }

# --- 3. Execute and Verify ---
if __name__ == "__main__":
    # argparse handles command-line arguments, providing help text and validation automatically.
    parser = argparse.ArgumentParser(description="Analyze chaos-rs logs and generate reports.")
    parser.add_argument("logfile", help="Path to the chaos log file to parse.")
    parser.add_argument("--json", help="Optional path to save results as a JSON file.")
    
    args = parser.parse_args()

    # Run our parser engine using the file path provided by the user
    try:
        parsed_results = parse_log_file(args.logfile)
    except FileNotFoundError:
        print(f"Error: The file '{args.logfile}' was not found.")
        exit(1)

    print("=== PARSER OUTPUT ===")
    for record in parsed_results:
        print(f"Test: {record.test_name} | Policy: {record.active_policy.name} | Status: {record.workload_status}")

    # Calculate and display summary
    stats = summarize(parsed_results)
    print("\n=== SUMMARY ===")
    for label, count in stats.items():
        print(f"{label}: {count}")

    # Write to JSON only if the user provided the --json flag
    if args.json:
        serializable_records = []
        for record in parsed_results:
            item = asdict(record)
            # Enums are not JSON serializable by default, so we use the .value (string)
            item["active_policy"] = record.active_policy.value
            serializable_records.append(item)

        with open(args.json, "w", encoding="utf-8") as jf:
            json.dump(serializable_records, jf, indent=4)
        print(f"\nResults saved to {args.json}")